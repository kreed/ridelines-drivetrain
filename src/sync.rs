use crate::convert::convert_fit_to_geojson;
use crate::intervals_client::{Activity, DownloadError, IntervalsClient};
use aws_sdk_s3::Client as S3Client;
use aws_sdk_s3::primitives::ByteStream;
use futures::stream::{self, StreamExt};
use sanitize_filename::sanitize;
use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use tokio::sync::Semaphore;
use tracing::{info, warn, error};

#[derive(Debug)]
pub struct DownloadStats {
    pub downloaded: usize,
    pub skipped_unchanged: usize,
    pub downloaded_empty: usize,
    pub failed: usize,
    pub deleted: usize,
}

pub async fn sync_activities(api_key: &str, athlete_id: &str, s3_client: &S3Client, s3_bucket: &str) {
    info!("Starting sync to S3 bucket: {s3_bucket}, athlete: {athlete_id}");

    // Create intervals client
    let client = IntervalsClient::new(api_key.to_string());

    // Get all activities
    let activities = match client.fetch_activities(athlete_id).await {
        Ok(activities) => activities,
        Err(e) => {
            error!("Error fetching activities: {e}");
            return;
        }
    };

    if activities.is_empty() {
        info!("No activities found for athlete {athlete_id}");
        return;
    }

    info!("Found {} activities for athlete {}", activities.len(), athlete_id);

    // Get all existing activity files in S3 (all are potentially orphaned initially)
    let s3_prefix = format!("athletes/{athlete_id}");
    let orphaned_files = match get_existing_activity_files_s3(s3_client, s3_bucket, &s3_prefix).await {
        Ok(files) => files,
        Err(e) => {
            warn!("Failed to get existing S3 files: {e}");
            HashSet::new()
        }
    };

    info!("Starting sync of {} activities", activities.len());

    let stats = Arc::new(Mutex::new(DownloadStats {
        downloaded: 0,
        skipped_unchanged: 0,
        downloaded_empty: 0,
        failed: 0,
        deleted: 0,
    }));

    // Limit concurrent downloads to avoid overwhelming the server
    let semaphore = Arc::new(Semaphore::new(5));
    let client = Arc::new(client);
    let s3_client = Arc::new(s3_client.clone());
    let s3_bucket = Arc::new(s3_bucket.to_string());
    let orphaned_files = Arc::new(Mutex::new(orphaned_files));

    // Process activities in parallel
    stream::iter(activities)
        .map(|activity| {
            let semaphore = semaphore.clone();
            let stats = stats.clone();
            let client = client.clone();
            let s3_client = s3_client.clone();
            let s3_bucket = s3_bucket.clone();
            let orphaned_files = orphaned_files.clone();
            let athlete_id = athlete_id.to_string();

            async move {
                let _permit = semaphore.acquire().await.unwrap();
                
                info!("Processing activity: {} (ID: {})", activity.name, activity.id);
                
                let geojson_key = generate_key(&activity, &athlete_id, "geojson");
                let stub_key = generate_key(&activity, &athlete_id, "stub");

                let geojson_exists = orphaned_files.lock().unwrap().contains(&geojson_key);
                let stub_exists = orphaned_files.lock().unwrap().contains(&stub_key);

                // Check for bad state (both files exist) - redownload
                if geojson_exists && stub_exists {
                    warn!("Both .geojson and .stub files exist for activity {}, redownloading", activity.id);
                } else if geojson_exists || stub_exists {
                    // One file exists, skip and remove both types from orphaned files
                    if let Ok(mut stats) = stats.lock() {
                        stats.skipped_unchanged += 1;
                    }
                    if let Ok(mut orphaned) = orphaned_files.lock() {
                        orphaned.remove(&geojson_key);
                        orphaned.remove(&stub_key);
                    }
                    return;
                }

                // Neither file exists, download activity
                
                // Helper function to write empty stub file to S3 and update stats
                let write_empty_file = || async {
                    let result = s3_client
                        .put_object()
                        .bucket(&*s3_bucket)
                        .key(&stub_key)
                        .body(ByteStream::from_static(b""))
                        .send()
                        .await;
                    
                    match result {
                        Ok(_) => {
                            if let Ok(mut stats) = stats.lock() {
                                stats.downloaded_empty += 1;
                            }
                            // Remove the stub file from orphaned files list
                            if let Ok(mut orphaned) = orphaned_files.lock() {
                                orphaned.remove(&stub_key);
                            }
                        }
                        Err(e) => {
                            error!("Failed to write stub file to S3 for activity {}: {}", activity.id, e);
                            if let Ok(mut stats) = stats.lock() {
                                stats.failed += 1;
                            }
                        }
                    }
                };

                // Download with retry logic handled by middleware
                match client.download_fit(&activity.id).await {
                    Ok(fit_data) => {
                        // Convert FIT to GeoJSON
                        match convert_fit_to_geojson(&fit_data, &activity).await {
                            Ok(Some(data)) => {
                                // Write GeoJSON file to S3
                                let result = s3_client
                                    .put_object()
                                    .bucket(&*s3_bucket)
                                    .key(&geojson_key)
                                    .body(ByteStream::from(data.into_bytes()))
                                    .content_type("application/geo+json")
                                    .send()
                                    .await;
                                
                                match result {
                                    Ok(_) => {
                                        if let Ok(mut stats) = stats.lock() {
                                            stats.downloaded += 1;
                                        }
                                        // Remove the geojson file from orphaned files list
                                        if let Ok(mut orphaned) = orphaned_files.lock() {
                                            orphaned.remove(&geojson_key);
                                        }
                                    }
                                    Err(e) => {
                                        error!("Failed to write GeoJSON file to S3 for activity {}: {}", activity.id, e);
                                        if let Ok(mut stats) = stats.lock() {
                                            stats.failed += 1;
                                        }
                                    }
                                }
                            }
                            Ok(None) => {
                                // No GPS data found in FIT file - write empty file
                                write_empty_file().await;
                            }
                            Err(e) => {
                                error!("Failed to convert FIT to GeoJSON for activity {}: {}", activity.id, e);
                                if let Ok(mut stats) = stats.lock() {
                                    stats.failed += 1;
                                }
                            }
                        }
                    }
                    Err(DownloadError::Http(status)) if status.as_u16() == 422 => {
                        // HTTP 422 usually means no GPS data available - write empty file
                        write_empty_file().await;
                    }
                    Err(e) => {
                        error!("Failed to download activity {}: {}", activity.id, e);
                        if let Ok(mut stats) = stats.lock() {
                            stats.failed += 1;
                        }
                    }
                }
            }
        })
        .buffer_unordered(5)
        .collect::<Vec<_>>()
        .await;

    info!("Activity processing complete");

    // Delete all remaining orphaned files from S3
    let orphaned_files_to_delete: Vec<String> = {
        let guard = orphaned_files.lock().unwrap();
        guard.iter().cloned().collect()
    };
    let deleted_count = orphaned_files_to_delete.len();
    for s3_key in &orphaned_files_to_delete {
        let result = s3_client
            .delete_object()
            .bucket(&**s3_bucket)
            .key(s3_key)
            .send()
            .await;
            
        if let Err(e) = result {
            warn!("Failed to delete orphaned S3 object {s3_key}: {e}");
        } else {
            info!("Deleted orphaned S3 object: {s3_key}");
        }
    }

    // Update deleted count in stats
    if let Ok(mut stats) = stats.lock() {
        stats.deleted = deleted_count;
    }

    // Extract final stats
    let final_stats = stats.lock().unwrap();
    info!("Sync summary:");
    info!("  Downloaded: {}", final_stats.downloaded);
    info!("  Skipped (unchanged): {}", final_stats.skipped_unchanged);
    info!("  Downloaded (empty/no GPS): {}", final_stats.downloaded_empty);
    info!("  Deleted (obsolete): {}", final_stats.deleted);
    info!("  Errors: {}", final_stats.failed);

}

async fn get_existing_activity_files_s3(s3_client: &S3Client, bucket: &str, prefix: &str) -> Result<HashSet<String>, Box<dyn std::error::Error>> {
    let files = s3_client
        .list_objects_v2()
        .bucket(bucket)
        .prefix(prefix)
        .into_paginator()
        .send()
        .try_collect()
        .await?
        .into_iter()
        .flat_map(|output| output.contents.unwrap_or_default())
        .filter_map(|object| object.key)
        .filter(|key| key.ends_with(".geojson") || key.ends_with(".stub"))
        .collect();

    Ok(files)
}


fn generate_key(activity: &Activity, athlete_id: &str, extension: &str) -> String {
    let date = parse_iso_date(&activity.start_date_local);
    let sanitized_name = sanitize(&activity.name);
    let sanitized_type = sanitize(&activity.activity_type);
    let distance_str = format_distance(activity.distance);
    let elapsed_time_str = format_elapsed_time(activity.elapsed_time);

    format!(
        "athletes/{athlete_id}/{date}-{sanitized_name}-{sanitized_type}-{distance_str}-{elapsed_time_str}-{}.{extension}",
        activity.id
    )
}

fn parse_iso_date(date_str: &str) -> String {
    // Extract just the date part (YYYY-MM-DD) from ISO datetime
    date_str.split('T').next().unwrap_or(date_str).to_string()
}

fn format_distance(distance: Option<f64>) -> String {
    match distance {
        Some(d) => format!("{:.2}km", d / 1000.0),
        None => "0.00km".to_string(),
    }
}

fn format_elapsed_time(elapsed_seconds: i64) -> String {
    let hours = elapsed_seconds / 3600;
    let minutes = (elapsed_seconds % 3600) / 60;
    let seconds = elapsed_seconds % 60;

    if hours > 0 {
        format!("{hours}h{minutes}m{seconds}s")
    } else if minutes > 0 {
        format!("{minutes}m{seconds}s")
    } else {
        format!("{seconds}s")
    }
}
