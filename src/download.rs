use crate::intervals_client::{Activity, DownloadError, IntervalsClient};
use indicatif::{ProgressBar, ProgressStyle};
use regex::Regex;
use sanitize_filename::sanitize;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::Write;
use std::path::Path;

#[derive(Debug)]
pub struct DownloadStats {
    pub total: usize,
    pub downloaded: usize,
    pub skipped_unchanged: usize,
    pub skipped_no_gps: HashSet<String>,
    pub failed: usize,
    pub deleted: usize,
}

pub async fn list_activities(api_key: &str, athlete_id: &str) {
    let client = IntervalsClient::new(api_key.to_string());
    match client.fetch_activities(athlete_id).await {
        Ok(activities) => {
            for activity in activities {
                println!("{activity:?}");
            }
        }
        Err(e) => {
            eprintln!("Error fetching activities: {e}");
        }
    }
}

pub async fn download_activity(api_key: &str, activity_id: &str, output_path: &Path) {
    let client = IntervalsClient::new(api_key.to_string());

    match client.download_gpx(activity_id, output_path).await {
        Ok(_) => {
            println!("GPX file saved to: {}", output_path.display());
        }
        Err(e) => {
            eprintln!("Error downloading GPX for activity {activity_id}: {e}");
        }
    }
}

pub async fn download_all_activities(api_key: &str, athlete_id: &str, output_dir: &Path) {
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

    // Get existing files in output directory
    let existing_files = get_existing_gpx_files(output_dir);
    let existing_activity_ids: HashSet<String> = existing_files.keys().cloned().collect();
    let current_activity_ids: HashSet<String> = activities.iter().map(|a| a.id.clone()).collect();

    // Find activities to delete (exist locally but not in current activity list)
    let activities_to_delete: Vec<String> = existing_activity_ids
        .difference(&current_activity_ids)
        .cloned()
        .collect();

    // Set up progress bar
    let pb = ProgressBar::new(activities.len() as u64);
    pb.set_style(
        ProgressStyle::default_bar()
            .template(
                "{spinner:.green} [{elapsed_precise}] [{wide_bar:.cyan/blue}] {pos}/{len} ({eta})",
            )
            .unwrap()
            .progress_chars("#>-"),
    );

    let mut stats = DownloadStats {
        total: activities.len(),
        downloaded: 0,
        skipped_unchanged: 0,
        skipped_no_gps: HashSet::new(),
        failed: 0,
        deleted: activities_to_delete.len(),
    };

    // Delete obsolete activities
    for activity_id in &activities_to_delete {
        if let Some(filename) = existing_files.get(activity_id) {
            let file_path = output_dir.join(filename);
            if let Err(e) = fs::remove_file(&file_path) {
                eprintln!("Warning: Failed to delete {filename}: {e}");
                stats.deleted -= 1;
            }
        }
    }

    // Process each activity
    for activity in activities {
        // Skip activities without GPS data. We rely on a heuristic here because there isn't any field in the activity list that tells explicitly if an activity has GPS data.
        let has_distance = activity.distance.unwrap_or(0.0) > 0.0;
        let is_zwift = activity.name.to_lowercase().contains("zwift");
        let skip_trainer_activity = activity.trainer == Some(true) && !is_zwift;
        if !has_distance || skip_trainer_activity {
            let expected_filename = generate_filename(&activity);
            stats.skipped_no_gps.insert(expected_filename);
            pb.inc(1);
            continue;
        }

        let expected_filename = generate_filename(&activity);
        let file_path = output_dir.join(&expected_filename);

        // Check if we need to download/update this activity
        let needs_download = if let Some(existing_filename) = existing_files.get(&activity.id) {
            // File exists, check if metadata has changed
            existing_filename != &expected_filename
        } else {
            // File doesn't exist
            true
        };

        if needs_download {
            // Remove old file if it exists with different name
            if let Some(existing_filename) = existing_files.get(&activity.id) {
                let old_path = output_dir.join(existing_filename);
                let _ = fs::remove_file(old_path);
            }

            // Download with retry logic handled by middleware
            match client.download_gpx(&activity.id, &file_path).await {
                Ok(_) => {
                    stats.downloaded += 1;
                }
                Err(DownloadError::Http(status)) if status.as_u16() == 422 => {
                    stats.skipped_no_gps.insert(expected_filename.clone());
                }
                Err(e) => {
                    eprintln!("Failed to download activity {}: {}", activity.id, e);
                    stats.failed += 1;
                }
            }
        } else {
            stats.skipped_unchanged += 1;
        }

        pb.inc(1);
    }

    pb.finish_with_message("Download complete");

    // Print summary
    println!("\nDownload Summary:");
    println!("Total activities: {}", stats.total);
    println!("Downloaded: {}", stats.downloaded);
    println!("Skipped (unchanged): {}", stats.skipped_unchanged);
    println!("Skipped (no GPS data): {}", stats.skipped_no_gps.len());
    println!("Failed: {}", stats.failed);
    println!("Deleted (obsolete): {}", stats.deleted);

    // Write skipped filenames to log file
    if !stats.skipped_no_gps.is_empty() {
        let log_path = output_dir.join("skipped_no_gps.log");
        match fs::File::create(&log_path) {
            Ok(mut file) => {
                for filename in &stats.skipped_no_gps {
                    writeln!(file, "{filename}").ok();
                }
            }
            Err(e) => {
                eprintln!("Warning: Failed to create skipped_no_gps.log: {e}");
            }
        }
    }
}

fn get_existing_gpx_files(output_dir: &Path) -> HashMap<String, String> {
    let mut files = HashMap::new();

    if let Ok(entries) = fs::read_dir(output_dir) {
        for entry in entries.flatten() {
            if let Some(filename) = entry.file_name().to_str() {
                if filename.ends_with(".gpx") {
                    if let Some(activity_id) = extract_activity_id_from_filename(filename) {
                        files.insert(activity_id, filename.to_string());
                    }
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
        "{}-{}-{}-{}-{}-{}.gpx",
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

fn extract_activity_id_from_filename(filename: &str) -> Option<String> {
    // Extract activity ID using regex with match group - all IDs start with 'i' and end before .gpx
    let re = Regex::new(r"-(i\d+)\.gpx$").unwrap();
    re.captures(filename).map(|caps| caps[1].to_string())
}
