use crate::intervals_client::{Activity, DownloadError, IntervalsClient};
use crate::convert::convert_gpx_to_geojson;
use futures::stream::{self, StreamExt};
use indicatif::{ProgressBar, ProgressStyle};
use sanitize_filename::sanitize;
use std::collections::HashSet;
use std::fs;
use std::io::Write;
use std::path::Path;
use std::sync::{Arc, Mutex};
use tokio::sync::Semaphore;

#[derive(Debug)]
pub struct DownloadStats {
    pub downloaded: usize,
    pub skipped_unchanged: usize,
    pub skipped_no_gps: HashSet<String>,
    pub failed: usize,
    pub deleted: usize,
}

pub async fn sync_activities(api_key: &str, athlete_id: &str, output_dir: &Path) {
    // Create output directory if it doesn't exist
    if let Err(e) = fs::create_dir_all(output_dir) {
        eprintln!("Error creating output directory: {e}");
        return;
    }

    // Create intervals client
    let client = IntervalsClient::new(api_key.to_string());

    // Get all activities
    let activities = match client.fetch_activities(athlete_id).await {
        Ok(activities) => activities,
        Err(e) => {
            eprintln!("Error fetching activities: {e}");
            return;
        }
    };

    if activities.is_empty() {
        println!("No activities found for athlete {athlete_id}");
        return;
    }

    // Get all existing GeoJSON files (all are potentially orphaned initially)
    let orphaned_files = get_existing_geojson_files(output_dir);

    // Set up progress bar
    let pb = ProgressBar::new(activities.len() as u64);
    pb.set_style(
        ProgressStyle::default_bar()
            .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} {msg}")
            .unwrap()
            .progress_chars("#>-"),
    );

    let stats = Arc::new(Mutex::new(DownloadStats {
        downloaded: 0,
        skipped_unchanged: 0,
        skipped_no_gps: HashSet::new(),
        failed: 0,
        deleted: 0,
    }));

    // Limit concurrent downloads to avoid overwhelming the server
    let semaphore = Arc::new(Semaphore::new(8));
    let pb = Arc::new(pb);
    let client = Arc::new(client);
    let output_dir = Arc::new(output_dir.to_path_buf());
    let orphaned_files = Arc::new(Mutex::new(orphaned_files));

    // Process activities in parallel
    stream::iter(activities)
        .map(|activity| {
            let semaphore = semaphore.clone();
            let pb = pb.clone();
            let stats = stats.clone();
            let client = client.clone();
            let output_dir = output_dir.clone();
            let orphaned_files = orphaned_files.clone();

            async move {
                let _permit = semaphore.acquire().await.unwrap();
                
                pb.set_message(format!("Processing {}", activity.name));
                
                let expected_filename = generate_filename(&activity);
                
                // Remove the expected filename from orphaned files (it's not orphaned)
                if let Ok(mut orphaned) = orphaned_files.lock() {
                    orphaned.remove(&expected_filename);
                }
                
                // Skip activities without GPS data
                let has_distance = activity.distance.unwrap_or(0.0) > 0.0;
                let is_zwift = activity.name.to_lowercase().contains("zwift");
                let skip_trainer_activity = activity.trainer == Some(true) && !is_zwift;
                if !has_distance || skip_trainer_activity {
                    if let Ok(mut stats) = stats.lock() {
                        stats.skipped_no_gps.insert(expected_filename);
                    }
                    pb.inc(1);
                    return;
                }

                let file_path = output_dir.join(&expected_filename);

                // Check if we need to download this activity (file doesn't exist)
                let needs_download = !file_path.exists();

                if needs_download {
                    // Download with retry logic handled by middleware
                    match client.download_gpx(&activity.id).await {
                        Ok(gpx_data) => {
                            // Convert GPX to GeoJSON
                            let geojson_data = match convert_gpx_to_geojson(&gpx_data).await {
                                Ok(data) => data,
                                Err(e) => {
                                    eprintln!("Failed to convert GPX to GeoJSON for activity {}: {}", activity.id, e);
                                    if let Ok(mut stats) = stats.lock() {
                                        stats.failed += 1;
                                    }
                                    return;
                                }
                            };

                            // Write GeoJSON file instead of GPX
                            let geojson_path = file_path.with_extension("geojson");
                            match fs::write(&geojson_path, geojson_data) {
                                Ok(_) => {
                                    if let Ok(mut stats) = stats.lock() {
                                        stats.downloaded += 1;
                                    }
                                }
                                Err(e) => {
                                    eprintln!("Failed to write GeoJSON file for activity {}: {}", activity.id, e);
                                    if let Ok(mut stats) = stats.lock() {
                                        stats.failed += 1;
                                    }
                                }
                            }
                        }
                        Err(DownloadError::Http(status)) if status.as_u16() == 422 => {
                            if let Ok(mut stats) = stats.lock() {
                                stats.skipped_no_gps.insert(expected_filename.clone());
                            }
                        }
                        Err(e) => {
                            eprintln!("Failed to download activity {}: {}", activity.id, e);
                            if let Ok(mut stats) = stats.lock() {
                                stats.failed += 1;
                            }
                        }
                    }
                } else if let Ok(mut stats) = stats.lock() {
                    stats.skipped_unchanged += 1;
                }

                pb.inc(1);
            }
        })
        .buffer_unordered(8)
        .collect::<Vec<_>>()
        .await;

    pb.finish_with_message("Sync complete!");

    // Delete all remaining orphaned files
    let orphaned_files_to_delete = orphaned_files.lock().unwrap();
    let deleted_count = orphaned_files_to_delete.len();
    for filename in orphaned_files_to_delete.iter() {
        let file_path = output_dir.join(filename);
        if let Err(e) = fs::remove_file(&file_path) {
            eprintln!("Warning: Failed to delete orphaned file {filename}: {e}");
        }
    }
    
    // Update deleted count in stats
    if let Ok(mut stats) = stats.lock() {
        stats.deleted = deleted_count;
    }

    // Extract final stats
    let final_stats = stats.lock().unwrap();
    println!("Sync summary:");
    println!("  Downloaded: {}", final_stats.downloaded);
    println!("  Skipped (unchanged): {}", final_stats.skipped_unchanged);
    println!("  Skipped (no GPS data): {}", final_stats.skipped_no_gps.len());
    println!("  Deleted (obsolete): {}", final_stats.deleted);
    println!("  Errors: {}", final_stats.failed);

    // Write skipped filenames to log file
    if !final_stats.skipped_no_gps.is_empty() {
        let log_path = output_dir.join("skipped_no_gps.log");
        match fs::File::create(&log_path) {
            Ok(mut file) => {
                for filename in &final_stats.skipped_no_gps {
                    writeln!(file, "{filename}").ok();
                }
            }
            Err(e) => {
                eprintln!("Warning: Failed to create skipped_no_gps.log: {e}");
            }
        }
    }
}

fn get_existing_geojson_files(output_dir: &Path) -> HashSet<String> {
    let mut files = HashSet::new();

    if let Ok(entries) = fs::read_dir(output_dir) {
        for entry in entries.flatten() {
            if let Some(filename) = entry.file_name().to_str() {
                if filename.ends_with(".geojson") {
                    files.insert(filename.to_string());
                }
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
        "{}-{}-{}-{}-{}-{}.geojson",
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