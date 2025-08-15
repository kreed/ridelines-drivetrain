use crate::activity_archive::ActivityArchiveManager;
use crate::convert::convert_fit_to_geojson;
use crate::intervals_client::{Activity, IntervalsClient};
use crate::metrics_helper;
use anyhow::Result;
use aws_sdk_s3::Client as S3Client;
use function_timer::time;
use futures::stream::{self, StreamExt};
use tracing::{error, info};

pub struct SyncJob {
    intervals_client: IntervalsClient,
    s3_client: S3Client,
    s3_bucket: String,
    athlete_id: String,
}

impl SyncJob {
    pub fn new(api_key: &str, athlete_id: &str, s3_client: S3Client, s3_bucket: &str) -> Self {
        Self {
            intervals_client: IntervalsClient::new(api_key.to_string()),
            s3_client,
            s3_bucket: s3_bucket.to_string(),
            athlete_id: athlete_id.to_string(),
        }
    }

    #[time("sync_activities_duration")]
    pub async fn sync_activities(&self) -> Result<()> {
        info!(
            "Starting simplified archive-based sync to S3 bucket: {}, athlete: {}",
            self.s3_bucket, self.athlete_id
        );

        // Load existing archive (or create empty if none exists)
        let mut existing_archive = match ActivityArchiveManager::load_existing(
            &self.s3_client,
            &self.s3_bucket,
            &self.athlete_id,
        )
        .await
        {
            Ok(archive) => Some(archive),
            Err(_) => {
                info!("No existing archive found, starting fresh");
                None
            }
        };

        // Create new empty archive for this sync
        let mut new_archive = ActivityArchiveManager::new_empty(self.athlete_id.clone());

        let activities = self
            .intervals_client
            .fetch_activities(&self.athlete_id)
            .await?;
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

            for activity in &activities {
                if new_archive.transfer_unchanged_entry(existing, activity) {
                    metrics_helper::increment_activities_skipped_unchanged(1);
                } else {
                    // Activity is new or changed, add to parallel processing queue
                    changed.push(activity.clone());
                }
            }

            info!(
                "Transferred {} unchanged, queued {} for parallel processing",
                activities.len() - changed.len(),
                changed.len()
            );

            changed
        } else {
            // No existing archive, all activities need processing
            info!(
                "No existing archive, processing all {} activities",
                activities.len()
            );
            activities
        };

        // Phase 2: Process new/changed activities in parallel
        if !changed_activities.is_empty() {
            let thread_results = stream::iter(changed_activities)
                .map(|activity| self.download_activity(activity))
                .buffer_unordered(5)
                .collect::<Vec<_>>()
                .await;

            // Phase 3: Merge all thread archives
            for thread_archive in thread_results {
                new_archive.extend(thread_archive);
            }
        }

        // Save the final merged archive
        new_archive
            .finalize(&self.s3_client, &self.s3_bucket)
            .await?;

        Ok(())
    }

    #[time("download_activity")]
    async fn download_activity(&self, activity: Activity) -> ActivityArchiveManager {
        // Create thread-local archive
        let mut thread_archive = ActivityArchiveManager::new_empty(self.athlete_id.clone());

        info!(
            "Processing activity: {} (ID: {})",
            activity.name, activity.id
        );
        // Download with retry logic handled by middleware
        match self.intervals_client.download_fit(&activity.id).await {
            Ok(Some(fit_data)) => {
                // Convert FIT to GeoJSON
                match convert_fit_to_geojson(&fit_data, &activity).await {
                    Ok(geojson) => {
                        if let Err(e) = thread_archive.add_new_activity(geojson.clone(), &activity)
                        {
                            error!("Failed to add activity {} to archive: {}", activity.id, e);
                            metrics_helper::increment_activities_failed(1);
                        } else if geojson.is_some() {
                            metrics_helper::increment_activities_with_gps(1);
                            metrics_helper::increment_activities_downloaded_new(1);
                        } else {
                            metrics_helper::increment_activities_without_gps(1);
                        }
                    }
                    Err(e) => {
                        error!(
                            "Failed to convert FIT to GeoJSON for activity {}: {}",
                            activity.id, e
                        );
                        metrics_helper::increment_activities_failed(1);
                    }
                }
            }
            Ok(None) => {
                // HTTP 422 response - no GPS data available, add without GeoJSON
                if let Err(e) = thread_archive.add_new_activity(None, &activity) {
                    error!("Failed to add activity {} to archive: {}", activity.id, e);
                    metrics_helper::increment_activities_failed(1);
                } else {
                    metrics_helper::increment_activities_without_gps(1);
                }
            }
            Err(e) => {
                error!("Failed to download activity {}: {}", activity.id, e);
                metrics_helper::increment_activities_failed(1);
            }
        }

        thread_archive
    }
}
