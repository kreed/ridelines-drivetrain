use crate::convert::convert_fit_to_geojson;
use crate::intervals_client::{Activity, DownloadError, IntervalsClient};
use futures::stream::{self, StreamExt};
use sanitize_filename::sanitize;
use std::collections::HashSet;
use std::fs;
use std::io::Write;
use std::path::Path;
use std::sync::{Arc, Mutex};
use tokio::sync::Semaphore;
use tracing::{info, warn, error};

#[derive(Debug)]
pub struct DownloadStats {
    pub downloaded: usize,
    pub skipped_unchanged: usize,
    pub downloaded_empty: HashSet<String>,
    pub failed: usize,
    pub deleted: usize,
}

pub async fn sync_activities(api_key: &str, athlete_id: &str, output_dir: &Path) {
    // Create output directory if it doesn't exist
    if let Err(e) = fs::create_dir_all(output_dir) {
        error!("Error creating output directory: {e}");
        return;
    }

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

    // Get all existing activity files (all are potentially orphaned initially)
    let orphaned_files = get_existing_activity_files(output_dir);

    info!("Starting sync of {} activities", activities.len());

    let stats = Arc::new(Mutex::new(DownloadStats {
        downloaded: 0,
        skipped_unchanged: 0,
        downloaded_empty: HashSet::new(),
        failed: 0,
        deleted: 0,
    }));

    // Limit concurrent downloads to avoid overwhelming the server
    let semaphore = Arc::new(Semaphore::new(5));
    let client = Arc::new(client);
    let output_dir = Arc::new(output_dir.to_path_buf());
    let orphaned_files = Arc::new(Mutex::new(orphaned_files));

    // Process activities in parallel
    stream::iter(activities)
        .map(|activity| {
            let semaphore = semaphore.clone();
            let stats = stats.clone();
            let client = client.clone();
            let output_dir = output_dir.clone();
            let orphaned_files = orphaned_files.clone();

            async move {
                let _permit = semaphore.acquire().await.unwrap();
                
                info!("Processing activity: {} (ID: {})", activity.name, activity.id);
                
                let expected_filename = generate_filename(&activity);
                
                // We'll remove from orphaned files after we know which file we created
                
                let geojson_path = output_dir.join(format!("{expected_filename}.geojson"));
                let stub_path = output_dir.join(format!("{expected_filename}.stub"));

                let geojson_exists = geojson_path.exists();
                let stub_exists = stub_path.exists();

                // Check for bad state (both files exist) - redownload
                if geojson_exists && stub_exists {
                    warn!("Both .geojson and .stub files exist for activity {}, redownloading", activity.id);
                } else if geojson_exists || stub_exists {
                    // One file exists, skip and remove both types from orphaned files
                    if let Ok(mut stats) = stats.lock() {
                        stats.skipped_unchanged += 1;
                    }
                    if let Ok(mut orphaned) = orphaned_files.lock() {
                        orphaned.remove(&format!("{expected_filename}.geojson"));
                        orphaned.remove(&format!("{expected_filename}.stub"));
                    }
                    return;
                }

                // Neither file exists, download activity
                
                // Helper function to write empty stub file and update stats
                let write_empty_file = || {
                    match fs::write(&stub_path, "") {
                        Ok(_) => {
                            if let Ok(mut stats) = stats.lock() {
                                stats.downloaded_empty.insert(expected_filename.clone());
                            }
                            // Remove the stub file from orphaned files list
                            if let Ok(mut orphaned) = orphaned_files.lock() {
                                orphaned.remove(&format!("{expected_filename}.stub"));
                            }
                        }
                        Err(e) => {
                            error!("Failed to write stub file for activity {}: {}", activity.id, e);
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
                                // Write GeoJSON file with data
                                match fs::write(&geojson_path, data) {
                                    Ok(_) => {
                                        if let Ok(mut stats) = stats.lock() {
                                            stats.downloaded += 1;
                                        }
                                        // Remove the geojson file from orphaned files list
                                        if let Ok(mut orphaned) = orphaned_files.lock() {
                                            orphaned.remove(&format!("{expected_filename}.geojson"));
                                        }
                                    }
                                    Err(e) => {
                                        error!("Failed to write GeoJSON file for activity {}: {}", activity.id, e);
                                        if let Ok(mut stats) = stats.lock() {
                                            stats.failed += 1;
                                        }
                                    }
                                }
                            }
                            Ok(None) => {
                                // No GPS data found in FIT file - write empty file
                                write_empty_file();
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
                        write_empty_file();
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

    // Delete all remaining orphaned files
    let orphaned_files_to_delete = orphaned_files.lock().unwrap();
    let deleted_count = orphaned_files_to_delete.len();
    for filename in orphaned_files_to_delete.iter() {
        let file_path = output_dir.join(filename);
        if let Err(e) = fs::remove_file(&file_path) {
            warn!("Failed to delete orphaned file {filename}: {e}");
        } else {
            info!("Deleted orphaned file: {filename}");
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
    info!(
        "  Downloaded (empty/no GPS): {}",
        final_stats.downloaded_empty.len()
    );
    info!("  Deleted (obsolete): {}", final_stats.deleted);
    info!("  Errors: {}", final_stats.failed);

    // Write empty filenames to log file
    if !final_stats.downloaded_empty.is_empty() {
        let log_path = output_dir.join("downloaded_empty.log");
        match fs::File::create(&log_path) {
            Ok(mut file) => {
                for filename in &final_stats.downloaded_empty {
                    writeln!(file, "{filename}").ok();
                }
            }
            Err(e) => {
                warn!("Failed to create downloaded_empty.log: {e}");
            }
        }
    }
}

fn get_existing_activity_files(output_dir: &Path) -> HashSet<String> {
    let mut files = HashSet::new();

    if let Ok(entries) = fs::read_dir(output_dir) {
        for entry in entries.flatten() {
            if let Some(filename) = entry.file_name().to_str()
                && (filename.ends_with(".geojson") || filename.ends_with(".stub"))
            {
                files.insert(filename.to_string());
            }
        }
    }

    files
}

fn generate_filename(activity: &Activity) -> String {
    let date = parse_iso_date(&activity.start_date_local);
    let sanitized_name = sanitize(&activity.name);
    let sanitized_type = sanitize(&activity.activity_type);
    let distance_str = format_distance(activity.distance);
    let elapsed_time_str = format_elapsed_time(activity.elapsed_time);

    format!(
        "{}-{}-{}-{}-{}-{}",
        date, sanitized_name, sanitized_type, distance_str, elapsed_time_str, activity.id
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
