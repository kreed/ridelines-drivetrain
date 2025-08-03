use clap::{Parser, Subcommand};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::collections::{HashMap, HashSet};
use indicatif::{ProgressBar, ProgressStyle};
use sanitize_filename::sanitize;

mod intervals_client;
use intervals_client::{Activity, IntervalsClient};

#[derive(Parser)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// List activities for an athlete
    List {
        /// Athlete ID
        #[arg(short, long)]
        id: String,
    },
    /// Download GPX file for a specific activity
    Download {
        /// Activity ID
        #[arg(short, long)]
        id: String,
        /// Path to save the GPX file
        #[arg(short, long)]
        path: PathBuf,
    },
    /// Download all GPX files for an athlete
    DownloadAll {
        /// Athlete ID
        #[arg(short, long)]
        id: String,
        /// Output directory for GPX files
        #[arg(short, long)]
        output_dir: PathBuf,
    },
}

#[tokio::main]
async fn main() {
    dotenv::dotenv().ok();
    let args = Cli::parse();

    let api_key = match env::var("INTERVALS_API_KEY") {
        Ok(key) => key,
        Err(_) => {
            eprintln!("Error: INTERVALS_API_KEY environment variable must be set");
            std::process::exit(1);
        }
    };

    match args.command {
        Commands::List { id } => list_activities(&api_key, &id).await,
        Commands::Download { id, path } => download_activity(&api_key, &id, &path).await,
        Commands::DownloadAll { id, output_dir } => download_all_activities(&api_key, &id, &output_dir).await,
    }
}

#[derive(Debug)]
struct DownloadStats {
    total: usize,
    downloaded: usize,
    skipped_unchanged: usize,
    skipped_no_gps: usize,
    failed: usize,
    deleted: usize,
}

async fn list_activities(api_key: &str, athlete_id: &str) {
    let client = IntervalsClient::new(api_key.to_string());
    match client.fetch_activities(athlete_id).await {
        Ok(activities) => {
            for activity in activities {
                println!("{activity:?}");
            }
        }
        Err(e) => {
            eprintln!("Error fetching activities: {}", e);
        }
    }
}

async fn download_activity(api_key: &str, activity_id: &str, output_path: &Path) {
    let client = IntervalsClient::new(api_key.to_string());
    
    match client.download_gpx(activity_id, output_path).await {
        Ok(_) => {
            println!("GPX file saved to: {}", output_path.display());
        }
        Err(e) => {
            eprintln!("Error downloading GPX for activity {}: {}", activity_id, e);
        }
    }
}

async fn download_all_activities(api_key: &str, athlete_id: &str, output_dir: &Path) {
    // Create output directory if it doesn't exist
    if let Err(e) = fs::create_dir_all(output_dir) {
        eprintln!("Error creating output directory: {}", e);
        return;
    }

    // Create intervals client
    let client = IntervalsClient::new(api_key.to_string());

    // Get all activities
    let activities = match client.fetch_activities(athlete_id).await {
        Ok(activities) => activities,
        Err(e) => {
            eprintln!("Error fetching activities: {}", e);
            return;
        }
    };

    if activities.is_empty() {
        println!("No activities found for athlete {}", athlete_id);
        return;
    }

    // Get existing files in output directory
    let existing_files = get_existing_gpx_files(output_dir);
    let existing_activity_ids: HashSet<String> = existing_files.keys().cloned().collect();
    let current_activity_ids: HashSet<String> = activities.iter().map(|a| a.id.clone()).collect();

    // Find activities to delete (exist locally but not in current activity list)
    let activities_to_delete: Vec<String> = existing_activity_ids.difference(&current_activity_ids).cloned().collect();
    
    // Set up progress bar
    let pb = ProgressBar::new(activities.len() as u64);
    pb.set_style(ProgressStyle::default_bar()
        .template("{spinner:.green} [{elapsed_precise}] [{wide_bar:.cyan/blue}] {pos}/{len} ({eta})")
        .unwrap()
        .progress_chars("#>-"));

    let mut stats = DownloadStats {
        total: activities.len(),
        downloaded: 0,
        skipped_unchanged: 0,
        skipped_no_gps: 0,
        failed: 0,
        deleted: activities_to_delete.len(),
    };

    // Delete obsolete activities
    for activity_id in &activities_to_delete {
        if let Some(filename) = existing_files.get(activity_id) {
            let file_path = output_dir.join(filename);
            if let Err(e) = fs::remove_file(&file_path) {
                eprintln!("Warning: Failed to delete {}: {}", filename, e);
                stats.deleted -= 1;
            }
        }
    }

    // Process each activity
    for activity in activities {
        // Skip activities without GPS data. We rely on a heuristic here (no distance || (trainer && no elevation gain) == no GPS). TODO: see if there's a better way to determine this
        if activity.distance.is_none() || (activity.trainer == Some(true) && activity.total_elevation_gain.is_none()) {
            stats.skipped_no_gps += 1;
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
                },
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
    println!("Skipped (no GPS data): {}", stats.skipped_no_gps);
    println!("Failed: {}", stats.failed);
    println!("Deleted (obsolete): {}", stats.deleted);
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
    let distance_str = format_distance(activity.distance);
    
    format!("{}-{}-{}-{}.gpx", date, sanitized_name, activity.id, distance_str)
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

fn extract_activity_id_from_filename(filename: &str) -> Option<String> {
    // Parse filename format: {date}-{name}-{activity_id}-{distance}.gpx
    let parts: Vec<&str> = filename.trim_end_matches(".gpx").split('-').collect();
    if parts.len() >= 4 {
        // Activity ID should be the third-to-last part (before distance)
        Some(parts[parts.len() - 2].to_string())
    } else {
        None
    }
}

