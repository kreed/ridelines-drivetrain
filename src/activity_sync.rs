use crate::activity_archive::ActivityArchiveManager;
use crate::convert::convert_fit_to_geojson;
use crate::intervals_client::{Activity, IntervalsClient};
use crate::metrics_helper;
use aws_sdk_s3::Client as S3Client;
use function_timer::time;
use std::sync::{Arc, Mutex};
use futures::stream::{self, StreamExt};
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
            &self.s3_client,
            &self.s3_bucket,
            &self.athlete_id,
        ).await {
            Ok(archive) => Some(archive),
            Err(_) => {
                info!("No existing archive found, starting fresh");
                None
            }
        };

        // Create new empty archive for this sync
        let mut new_archive = ActivityArchiveManager::new_empty(
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

        // Phase 1: Transfer unchanged activities and collect new/changed for parallel processing
        let changed_activities = if let Some(ref mut existing) = existing_archive {
            let mut changed = Vec::new();
            
            let num_activities = activities.len();
            for activity in &activities {
                if new_archive.transfer_unchanged_entry(existing, activity) {
                    if let Ok(mut stats) = self.stats.lock() {
                        stats.skipped_unchanged += 1;
                    }
                } else {
                    // Activity is new or changed, add to parallel processing queue
                    changed.push(activity.clone());
                }
            }
            
            info!(
                "Transferred {} unchanged, queued {} for parallel processing",
                num_activities - changed.len(),
                changed.len()
            );
            
            changed
        } else {
            // No existing archive, all activities need processing
            info!("No existing archive, processing all {} activities", activities.len());
            activities
        };

        // Phase 2: Process new/changed activities in parallel with per-thread archives
        if !changed_activities.is_empty() {
            let thread_results = stream::iter(changed_activities)
                .map(|activity| {
                    self.process_activity_with_thread_archive(activity)
                })
                .buffer_unordered(5)
                .collect::<Vec<_>>()
                .await;

            // Phase 3: Merge all thread archives and stats into the main archive
            for (thread_archive, thread_stats) in thread_results {
                self.merge_thread_results(&mut new_archive, thread_archive, thread_stats);
            }
        }

        // Save the final merged archive
        new_archive.finalize(&self.s3_client, &self.s3_bucket).await?;

        self.report_results();

        Ok(())
    }


    async fn process_activity_with_thread_archive(
        &self,
        activity: Activity,
    ) -> (ActivityArchiveManager, DownloadStats) {
        let _permit = self.semaphore.acquire().await.unwrap();

        info!(
            "Processing activity in thread: {} (ID: {})",
            activity.name, activity.id
        );

        // Create thread-local archive and stats
        let mut thread_archive = ActivityArchiveManager::new_empty(
            self.athlete_id.clone(),
        );

        let mut thread_stats = DownloadStats {
            downloaded: 0,
            skipped_unchanged: 0,
            downloaded_empty: 0,
            failed: 0,
        };

        self.download_activity(&activity, &mut thread_archive, &mut thread_stats).await;

        (thread_archive, thread_stats)
    }

    fn merge_thread_results(
        &self,
        main_archive: &mut ActivityArchiveManager,
        thread_archive: ActivityArchiveManager,
        thread_stats: DownloadStats,
    ) {
        // Merge archive entries using the extend method
        main_archive.extend(thread_archive);

        // Merge stats
        if let Ok(mut stats) = self.stats.lock() {
            stats.downloaded += thread_stats.downloaded;
            stats.skipped_unchanged += thread_stats.skipped_unchanged;
            stats.downloaded_empty += thread_stats.downloaded_empty;
            stats.failed += thread_stats.failed;
        }
    }

    #[time("download_activity")]
    async fn download_activity(
        &self,
        activity: &Activity,
        thread_archive: &mut ActivityArchiveManager,
        thread_stats: &mut DownloadStats,
    ) {
        // Download with retry logic handled by middleware
        match self.intervals_client.download_fit(&activity.id).await {
            Ok(Some(fit_data)) => {
                // Convert FIT to GeoJSON
                match convert_fit_to_geojson(&fit_data, activity).await {
                    Ok(Some(geojson_data)) => {
                        // Add activity with GeoJSON data to thread archive
                        if let Err(e) = thread_archive
                            .add_new_activity(Some(geojson_data), activity)
                            .await
                        {
                            error!("Failed to add activity {} to archive: {}", activity.id, e);
                            thread_stats.failed += 1;
                        } else {
                            thread_stats.downloaded += 1;
                        }
                    }
                    Ok(None) => {
                        // No GPS data found in FIT file - add without GeoJSON
                        if let Err(e) = thread_archive.add_new_activity(None, activity).await {
                            error!("Failed to add activity {} to archive: {}", activity.id, e);
                            thread_stats.failed += 1;
                        } else {
                            thread_stats.downloaded_empty += 1;
                        }
                    }
                    Err(e) => {
                        error!(
                            "Failed to convert FIT to GeoJSON for activity {}: {}",
                            activity.id, e
                        );
                        thread_stats.failed += 1;
                    }
                }
            }
            Ok(None) => {
                // HTTP 422 response - no GPS data available, add without GeoJSON
                if let Err(e) = thread_archive.add_new_activity(None, activity).await {
                    error!("Failed to add activity {} to archive: {}", activity.id, e);
                    thread_stats.failed += 1;
                } else {
                    thread_stats.downloaded_empty += 1;
                }
            }
            Err(e) => {
                error!("Failed to download activity {}: {}", activity.id, e);
                thread_stats.failed += 1;
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
