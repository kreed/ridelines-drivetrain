use super::{ActivityIndex, ActivitySync};
use crate::fit_converter::convert_fit_to_geojson;
use anyhow::Result;
use function_timer::time;
use futures::stream::{self, StreamExt};
use ridelines_drivetrain::common::intervals_client::Activity;
use ridelines_drivetrain::common::metrics;
use tracing::{error, info};

impl ActivitySync {
    #[time("sync_activities_duration")]
    pub async fn sync_activities(&self) -> Result<Option<std::path::PathBuf>> {
        // Phase 1: Load existing index (metadata only, not full archive)
        let existing_index = match self.download_index().await {
            Ok(index) => Some(index),
            Err(_) => {
                info!("No existing index found, starting fresh");
                None
            }
        };

        let activities = self.intervals_client.fetch_activities().await?;
        if activities.is_empty() {
            info!("No activities found for user {}", self.user_id);
            return Ok(None);
        }

        info!(
            "Found {} activities for user {}",
            activities.len(),
            self.user_id
        );

        // Phase 2: Identify unchanged vs new/changed activities and create copied index
        let (copied_index, changed_activities, has_changes) =
            if let Some(ref existing) = existing_index {
                let mut copied = ActivityIndex::new_empty(self.user_id.clone());
                let mut changed = Vec::new();

                for activity in &activities {
                    if existing.try_copy(activity, &mut copied) {
                        metrics::increment_activities_skipped_unchanged(1);
                    } else {
                        // Activity is new or changed, add to parallel processing queue
                        changed.push(activity.clone());
                    }
                }

                // Check if activities were deleted (existed before but not in current list)
                let activities_deleted = existing.total_activities() > copied.total_activities();
                let has_changes = !changed.is_empty() || activities_deleted;

                if activities_deleted {
                    info!(
                        "Detected {} deleted activities",
                        existing.total_activities() - copied.total_activities()
                    );
                }

                info!(
                    "Keeping {} unchanged activities, queued {} for download.",
                    copied.total_activities(),
                    changed.len()
                );

                (copied, changed, has_changes)
            } else {
                // No existing index, all activities need processing
                info!(
                    "No existing index, processing all {} activities",
                    activities.len()
                );
                let empty_index = ActivityIndex::new_empty(self.user_id.clone());
                (empty_index, activities, true) // Always has changes when starting fresh
            };

        // Short circuit: if no changes detected, skip archive upload and tile generation
        if !has_changes {
            info!("No activity changes detected, skipping archive upload and tile generation");
            return Ok(None);
        }

        // Phase 3: Create subdirectory for changed activities and process them in parallel
        let changed_activities_dir = self.work_dir.join("activities");
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

        Ok(Some(geojson_file_path))
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

    async fn process_activity(&self, activity: Activity, temp_dir: &std::path::Path) -> Result<()> {
        info!(
            "Processing activity: {} (ID: {})",
            activity.name, activity.id
        );

        // Compute activity hash once
        let activity_hash = activity.compute_hash();

        // Download and convert activity
        match self.download_and_convert_activity(&activity).await {
            Ok(Some(geojson)) => {
                // Write GeoJSON directly to temp file with hash in filename
                let temp_file_path = temp_dir.join(format!(
                    "activity_{}_{}.geojson",
                    activity.id, activity_hash
                ));
                match std::fs::write(&temp_file_path, &geojson) {
                    Ok(_) => {
                        metrics::increment_activities_with_gps(1);
                        metrics::increment_activities_downloaded_new(1);
                        info!("Saved GeoJSON to: {}", temp_file_path.display());
                    }
                    Err(e) => {
                        error!(
                            "Failed to write activity {} to temp file: {}",
                            activity.id, e
                        );
                        metrics::increment_activities_failed(1);
                    }
                }
            }
            Ok(None) => {
                // No GPS data, create empty stub file with hash in filename
                let stub_file_path =
                    temp_dir.join(format!("activity_{}_{}.stub", activity.id, activity_hash));

                match std::fs::write(&stub_file_path, "") {
                    Ok(_) => {
                        metrics::increment_activities_without_gps(1);
                        info!("Saved empty stub to: {}", stub_file_path.display());
                    }
                    Err(e) => {
                        error!("Failed to write stub for activity {}: {}", activity.id, e);
                        metrics::increment_activities_failed(1);
                    }
                }
            }
            Err(e) => {
                error!("Failed to download/convert activity {}: {}", activity.id, e);
                metrics::increment_activities_failed(1);
            }
        }

        Ok(())
    }
}
