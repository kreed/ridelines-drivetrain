use crate::activity_archive::{ActivityArchive, ActivityArchiveManager};
use crate::convert::convert_fit_to_geojson;
use crate::intervals_client::{Activity, IntervalsClient};
use crate::metrics_helper;
use aws_sdk_s3::Client as S3Client;
use function_timer::time;
use std::sync::{Arc, Mutex};
use tokio::sync::Semaphore;
use tracing::{error, info};

#[derive(Debug)]
pub struct DownloadStats {
    pub downloaded: usize,
    pub skipped_unchanged: usize,
    pub downloaded_empty: usize,
    pub failed: usize,
}

pub struct SyncJob {
    intervals_client: IntervalsClient,
    s3_client: S3Client,
    s3_bucket: String,
    athlete_id: String,
    stats: Arc<Mutex<DownloadStats>>,
    semaphore: Arc<Semaphore>,
}

impl SyncJob {
    pub fn new(api_key: &str, athlete_id: &str, s3_client: S3Client, s3_bucket: &str) -> Self {
        Self {
            intervals_client: IntervalsClient::new(api_key.to_string()),
            s3_client,
            s3_bucket: s3_bucket.to_string(),
            athlete_id: athlete_id.to_string(),
            stats: Arc::new(Mutex::new(DownloadStats {
                downloaded: 0,
                skipped_unchanged: 0,
                downloaded_empty: 0,
                failed: 0,
            })),
            semaphore: Arc::new(Semaphore::new(5)),
        }
    }

    #[time("sync_activities_duration")]
    pub async fn sync_activities(&self) -> Result<(), Box<dyn std::error::Error>> {
        info!(
            "Starting simplified archive-based sync to S3 bucket: {}, athlete: {}",
            self.s3_bucket, self.athlete_id
        );

        // Load existing archive (or create empty if none exists)
        let mut existing_archive = match ActivityArchiveManager::load_existing(
            self.s3_client.clone(),
            self.s3_bucket.clone(),
            self.athlete_id.clone(),
        ).await {
            Ok(archive) => Some(archive),
            Err(_) => {
                info!("No existing archive found, starting fresh");
                None
            }
        };

        // Create new empty archive for this sync
        let mut new_archive = ActivityArchiveManager::new_empty(
            self.s3_client.clone(),
            self.s3_bucket.clone(),
            self.athlete_id.clone(),
        );

        let activities = self.intervals_client.fetch_activities(&self.athlete_id).await?;
        if activities.is_empty() {
            info!("No activities found for athlete {}", self.athlete_id);
            return Ok(());
        }

        info!(
            "Found {} activities for athlete {}",
            activities.len(),
            self.athlete_id
        );

        // Process each activity: transfer unchanged or download new/changed
        for activity in activities {
            self.process_single_activity(
                activity,
                &mut new_archive,
                &mut existing_archive,
            ).await;
        }

        // Save the new archive (completely replaces old one)
        new_archive.finalize().await?;

        self.report_results();

        Ok(())
    }


    async fn process_single_activity(
        &self,
        activity: Activity,
        new_archive: &mut ActivityArchiveManager,
        existing_archive: &mut Option<ActivityArchive>,
    ) {
        let _permit = self.semaphore.acquire().await.unwrap();

        info!(
            "Processing activity: {} (ID: {})",
            activity.name, activity.id
        );

        // Try to transfer unchanged entry from existing archive
        if let Some(existing) = existing_archive {
            if new_archive.transfer_unchanged_entry(existing, &activity) {
                if let Ok(mut stats) = self.stats.lock() {
                    stats.skipped_unchanged += 1;
                }
                return;
            }
        }

        // Activity is new or changed, download and process
        self.download_and_add_activity(&activity, new_archive).await;
    }

    #[time("download_activity")]
    async fn download_and_add_activity(
        &self,
        activity: &Activity,
        new_archive: &mut ActivityArchiveManager,
    ) {
        // Download with retry logic handled by middleware
        match self.intervals_client.download_fit(&activity.id).await {
            Ok(Some(fit_data)) => {
                // Convert FIT to GeoJSON
                match convert_fit_to_geojson(&fit_data, activity).await {
                    Ok(Some(geojson_data)) => {
                        // Add activity with GeoJSON data to new archive
                        if let Err(e) = new_archive
                            .add_new_activity(Some(geojson_data), activity)
                            .await
                        {
                            error!("Failed to add activity {} to archive: {}", activity.id, e);
                            if let Ok(mut stats) = self.stats.lock() {
                                stats.failed += 1;
                            }
                        } else if let Ok(mut stats) = self.stats.lock() {
                            stats.downloaded += 1;
                        }
                    }
                    Ok(None) => {
                        // No GPS data found in FIT file - add without GeoJSON
                        if let Err(e) = new_archive.add_new_activity(None, activity).await {
                            error!("Failed to add activity {} to archive: {}", activity.id, e);
                            if let Ok(mut stats) = self.stats.lock() {
                                stats.failed += 1;
                            }
                        } else if let Ok(mut stats) = self.stats.lock() {
                            stats.downloaded_empty += 1;
                        }
                    }
                    Err(e) => {
                        error!(
                            "Failed to convert FIT to GeoJSON for activity {}: {}",
                            activity.id, e
                        );
                        if let Ok(mut stats) = self.stats.lock() {
                            stats.failed += 1;
                        }
                    }
                }
            }
            Ok(None) => {
                // HTTP 422 response - no GPS data available, add without GeoJSON
                if let Err(e) = new_archive.add_new_activity(None, activity).await {
                    error!("Failed to add activity {} to archive: {}", activity.id, e);
                    if let Ok(mut stats) = self.stats.lock() {
                        stats.failed += 1;
                    }
                } else if let Ok(mut stats) = self.stats.lock() {
                    stats.downloaded_empty += 1;
                }
            }
            Err(e) => {
                error!("Failed to download activity {}: {}", activity.id, e);
                if let Ok(mut stats) = self.stats.lock() {
                    stats.failed += 1;
                }
            }
        }
    }

    fn report_results(&self) {
        let final_stats = self.stats.lock().unwrap();
        metrics_helper::set_activities_with_gps_count(final_stats.downloaded as u64);
        metrics_helper::set_activities_without_gps_count(final_stats.downloaded_empty as u64);
        metrics_helper::set_activities_skipped_unchanged(final_stats.skipped_unchanged as u64);
        metrics_helper::set_activities_downloaded_new(final_stats.downloaded as u64);
    }
}
