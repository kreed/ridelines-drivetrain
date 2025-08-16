use super::{ActivitySync, ActivityIndex};
use crate::convert::convert_fit_to_geojson;
use crate::intervals_client::Activity;
use crate::metrics_helper;
use anyhow::Result;
use function_timer::time;
use futures::stream::{self, StreamExt};
use tracing::{error, info};

impl ActivitySync {
    #[time("sync_activities_duration")]
    pub async fn sync_activities(&self) -> Result<String> {
        // Phase 1: Load existing index (metadata only, not full archive)
        let existing_index = match self.download_index().await {
            Ok(index) => Some(index),
            Err(_) => {
                info!("No existing index found, starting fresh");
                None
            }
        };

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

        // Phase 2: Identify unchanged vs new/changed activities and create copied index
        let (copied_index, changed_activities) =
            if let Some(ref existing) = existing_index {
                let mut copied = ActivityIndex::new_empty(self.athlete_id.clone());
                let mut changed = Vec::new();

                for activity in &activities {
                    if existing.try_copy(activity, &mut copied) {
                        metrics_helper::increment_activities_skipped_unchanged(1);
                    } else {
                        // Activity is new or changed, add to parallel processing queue
                        changed.push(activity.clone());
                    }
                }

                info!(
                    "Keeping {} unchanged activities, queued {} for download.",
                    copied.total_activities(),
                    changed.len()
                );

                (copied, changed)
            } else {
                // No existing index, all activities need processing
                info!(
                    "No existing index, processing all {} activities",
                    activities.len()
                );
                let empty_index = ActivityIndex::new_empty(self.athlete_id.clone());
                (empty_index, activities)
            };

        // Phase 3: Create temp directory and process new/changed activities in parallel
        let changed_activities_dir = format!("/tmp/sync_{}", self.athlete_id);
        std::fs::create_dir_all(&changed_activities_dir)?;

        if !changed_activities.is_empty() {
            stream::iter(changed_activities)
                .map(|activity| self.process_activity(activity, &changed_activities_dir))
                .buffer_unordered(5)
                .collect::<Vec<_>>()
                .await;
        }

        // Phase 4: Finalize archive by streaming existing + new activities from temp dir
        let geojson_file_path = self
            .finalize_archive(&changed_activities_dir, copied_index)
            .await?;

        Ok(geojson_file_path)
    }

    async fn download_and_convert_activity(&self, activity: &Activity) -> Result<Option<String>> {
        match self.intervals_client.download_fit(&activity.id).await {
            Ok(Some(fit_data)) => convert_fit_to_geojson(&fit_data, activity).await,
            Ok(None) => Ok(None),
            Err(e) => {
                error!("Failed to download activity {}: {}", activity.id, e);
                Err(e.into())
            }
        }
    }

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
                let temp_file_path = format!(
                    "{}/activity_{}_{}.geojson",
                    temp_dir, activity.id, activity_hash
                );
                match std::fs::write(&temp_file_path, &geojson) {
                    Ok(_) => {
                        metrics_helper::increment_activities_with_gps(1);
                        metrics_helper::increment_activities_downloaded_new(1);
                        info!("Saved GeoJSON to: {}", temp_file_path);
                    }
                    Err(e) => {
                        error!(
                            "Failed to write activity {} to temp file: {}",
                            activity.id, e
                        );
                        metrics_helper::increment_activities_failed(1);
                    }
                }
            }
            Ok(None) => {
                // No GPS data, create empty stub file with hash in filename
                let stub_file_path = format!(
                    "{}/activity_{}_{}.stub",
                    temp_dir, activity.id, activity_hash
                );

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

        Ok(())
    }
}
