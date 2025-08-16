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
    pub async fn sync_activities(&self) -> Result<String> {
        info!(
            "Starting memory-efficient archive-based sync to S3 bucket: {}, athlete: {}",
            self.s3_bucket, self.athlete_id
        );

        // Phase 1: Load existing index (metadata only, not full archive)
        let existing_index = match ActivityArchiveManager::load_existing_index(
            &self.s3_client,
            &self.s3_bucket,
            &self.athlete_id,
        )
        .await
        {
            Ok(index) => Some(index),
            Err(_) => {
                info!("No existing index found, starting fresh");
                None
            }
        };

        // Create new archive manager for finalization
        let new_archive = ActivityArchiveManager::new_empty(self.athlete_id.clone());

        let activities = self
            .intervals_client
            .fetch_activities(&self.athlete_id)
            .await?;
        if activities.is_empty() {
            info!("No activities found for athlete {}", self.athlete_id);
            return Ok(String::new());
        }

        info!(
            "Found {} activities for athlete {}",
            activities.len(),
            self.athlete_id
        );

        // Phase 2: Identify unchanged vs new/changed activities
        let (unchanged_activity_ids, changed_activities) = if let Some(ref existing) = existing_index {
            let mut unchanged_ids = Vec::new();
            let mut changed = Vec::new();

            for activity in &activities {
                if ActivityArchiveManager::is_activity_unchanged(existing, activity) {
                    unchanged_ids.push(activity.id.clone());
                    metrics_helper::increment_activities_skipped_unchanged(1);
                } else {
                    // Activity is new or changed, add to parallel processing queue
                    changed.push(activity.clone());
                }
            }

            info!(
                "Keeping {} unchanged activities, queued {} for parallel processing",
                unchanged_ids.len(),
                changed.len()
            );

            (unchanged_ids, changed)
        } else {
            // No existing index, all activities need processing
            info!(
                "No existing index, processing all {} activities",
                activities.len()
            );
            (Vec::new(), activities)
        };

        // Phase 3: Create temp directory and process new/changed activities in parallel
        let temp_dir = format!("/tmp/sync_{}", self.athlete_id);
        std::fs::create_dir_all(&temp_dir)?;
        
        if !changed_activities.is_empty() {
            let results = stream::iter(changed_activities)
                .map(|activity| self.process_activity(activity, &temp_dir))
                .buffer_unordered(5)
                .collect::<Vec<_>>()
                .await;

            // Check for any failures
            for result in results {
                if let Err(e) = result {
                    error!("Activity download failed: {}", e);
                }
            }
        }

        // Phase 4: Finalize archive by streaming existing + new activities from temp dir
        let geojson_file_path = new_archive
            .finalize_archive(&unchanged_activity_ids, &temp_dir, &self.s3_client, &self.s3_bucket)
            .await?;

        Ok(geojson_file_path)
    }

    #[time("download_and_convert_activity")]
    async fn download_and_convert_activity(&self, activity: &Activity) -> Result<Option<String>> {
        info!(
            "Downloading and converting activity: {} (ID: {})",
            activity.name, activity.id
        );
        
        // Download with retry logic handled by middleware
        match self.intervals_client.download_fit(&activity.id).await {
            Ok(Some(fit_data)) => {
                info!(
                    "Converting FIT data for activity: {} (ID: {})",
                    activity.name, activity.id
                );
                
                // Convert FIT to GeoJSON
                convert_fit_to_geojson(&fit_data, activity).await
            }
            Ok(None) => {
                // HTTP 422 response - no GPS data available
                Ok(None)
            }
            Err(e) => {
                error!("Failed to download activity {}: {}", activity.id, e);
                Err(e.into())
            }
        }
    }

    #[time("process_activity")]
    async fn process_activity(&self, activity: Activity, temp_dir: &str) -> Result<()> {
        info!(
            "Processing activity: {} (ID: {})",
            activity.name, activity.id
        );
        
        // Compute activity hash once
        let activity_hash = activity.compute_hash();
        
        // Download and convert activity
        match self.download_and_convert_activity(&activity).await {
            Ok(Some(geojson)) => {
                info!(
                    "Writing GeoJSON for activity: {} (ID: {})",
                    activity.name, activity.id
                );

                // Write GeoJSON directly to temp file with hash in filename
                let temp_file_path = format!("{}/activity_{}_{}.geojson", temp_dir, activity.id, activity_hash);
                match std::fs::write(&temp_file_path, &geojson) {
                    Ok(_) => {
                        metrics_helper::increment_activities_with_gps(1);
                        metrics_helper::increment_activities_downloaded_new(1);
                        info!("Saved GeoJSON to: {}", temp_file_path);
                    }
                    Err(e) => {
                        error!("Failed to write activity {} to temp file: {}", activity.id, e);
                        metrics_helper::increment_activities_failed(1);
                    }
                }
            }
            Ok(None) => {
                // No GPS data, create empty stub file with hash in filename
                let stub_file_path = format!("{}/activity_{}_{}.stub", temp_dir, activity.id, activity_hash);
                
                match std::fs::write(&stub_file_path, "") {
                    Ok(_) => {
                        metrics_helper::increment_activities_without_gps(1);
                        info!("Saved empty stub to: {}", stub_file_path);
                    }
                    Err(e) => {
                        error!("Failed to write stub for activity {}: {}", activity.id, e);
                        metrics_helper::increment_activities_failed(1);
                    }
                }
            }
            Err(e) => {
                error!("Failed to download/convert activity {}: {}", activity.id, e);
                metrics_helper::increment_activities_failed(1);
            }
        }

        info!("Finished activity {}", activity.id);
        Ok(())
    }
}
