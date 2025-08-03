use clap::{Parser, Subcommand};
use base64::prelude::*;
use serde::Deserialize;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::collections::{HashMap, HashSet};
use indicatif::{ProgressBar, ProgressStyle};
use sanitize_filename::sanitize;
use reqwest_middleware::{ClientBuilder, ClientWithMiddleware};
use reqwest_retry::{RetryTransientMiddleware, policies::ExponentialBackoff};

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
    /// Get GPX file for a specific activity
    Get {
        /// Activity ID
        #[arg(short, long)]
        id: String,
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
        Commands::Get { id } => get_activity(&api_key, &id).await,
        Commands::DownloadAll { id, output_dir } => download_all_activities(&api_key, &id, &output_dir).await,
    }
}

const ENDPOINT: &str = "https://intervals.icu";

// id,start_date_local,name,type,moving_time,distance,elapsed_time,total_elevation_gain,max_speed,average_speed,has_heartrate,max_heartrate,average_heartrate,average_cadence,calories,device_watts,icu_average_watts,icu_normalized_watts,icu_joules,icu_intensity,icu_training_load,icu_training_load_edited,icu_rpe,pace,icu_fatigue,icu_fitness,icu_eftp,icu_variability,icu_efficiency,trainer,commute,race,sub_type,icu_ftp,icu_w_prime,threshold_pace,power_load,hr_load,pace_load,icu_resting_hr,lthr,hr_z1,hr_z2,hr_z3,hr_z4,hr_z5,hr_z6,hr_max,hr_z1_secs,hr_z2_secs,hr_z3_secs,hr_z4_secs,hr_z5_secs,hr_z6_secs,hr_z7_secs,z1_secs,z2_secs,z3_secs,z4_secs,z5_secs,z6_secs,z7_secs,sweet_spot_secs,icu_weight,icu_ignore_power,icu_ignore_hr,icu_ignore_time,icu_recording_time,icu_warmup_time,icu_cooldown_time,icu_hrrc,icu_hrrc_start_bpm,icu_power_spike_threshold,icu_pm_ftp,icu_pm_cp,icu_pm_w_prime,icu_pm_p_max,start_date,icu_sync_date,timezone,file_type,external_id,compliance,gear,description
#[derive(Debug, Deserialize, Clone)]
struct Activity {
    id: String,
    name: String,
    start_date_local: String,
    distance: Option<f64>,
    total_elevation_gain: Option<f64>,
    trainer: Option<bool>,
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
    let path = format!("{ENDPOINT}/api/v1/athlete/{athlete_id}/activities.csv");
    let client = reqwest::Client::new();
    let body = client.get(path)
        .header("Authorization", auth_header(api_key))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();

    let mut rdr = csv::Reader::from_reader(body.as_bytes());
    for result in rdr.deserialize() {
        let activity: Activity = result.unwrap();
        println!("{activity:?}");
    }
}

async fn get_activity(api_key: &str, activity_id: &str) {
    let path = format!("{ENDPOINT}/api/v1/activity/{activity_id}/gpx-file");
    let client = reqwest::Client::new();
    let body = client.get(path)
        .header("Authorization", auth_header(api_key))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    println!("{body:?}");
}

async fn download_all_activities(api_key: &str, athlete_id: &str, output_dir: &Path) {
    // Create output directory if it doesn't exist
    if let Err(e) = fs::create_dir_all(output_dir) {
        eprintln!("Error creating output directory: {}", e);
        return;
    }

    // Create client with retry middleware
    let retry_policy = ExponentialBackoff::builder().build_with_max_retries(2);
    let client = ClientBuilder::new(reqwest::Client::new())
        .with(RetryTransientMiddleware::new_with_policy(retry_policy))
        .build();

    // Get all activities
    let activities = match fetch_activities(api_key, athlete_id, &client).await {
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
            match download_gpx(api_key, &activity.id, &file_path, &client).await {
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

async fn fetch_activities(api_key: &str, athlete_id: &str, client: &ClientWithMiddleware) -> Result<Vec<Activity>, Box<dyn std::error::Error>> {
    let path = format!("{ENDPOINT}/api/v1/athlete/{athlete_id}/activities.csv");
    let body = client.get(path)
        .header("Authorization", auth_header(api_key))
        .send()
        .await?
        .text()
        .await?;

    let mut rdr = csv::Reader::from_reader(body.as_bytes());
    let mut activities = Vec::new();
    
    for result in rdr.deserialize() {
        let activity: Activity = result?;
        activities.push(activity);
    }
    
    Ok(activities)
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

async fn download_gpx(api_key: &str, activity_id: &str, file_path: &Path, client: &ClientWithMiddleware) -> Result<(), Box<dyn std::error::Error>> {
    let path = format!("{ENDPOINT}/api/v1/activity/{activity_id}/gpx-file");
    let response = client.get(path)
        .header("Authorization", auth_header(api_key))
        .send()
        .await?;
    
    if !response.status().is_success() {
        return Err(format!("HTTP {}: Failed to download GPX for activity {}", response.status(), activity_id).into());
    }
    
    let body = response.text().await?;
    fs::write(file_path, body)?;
    
    Ok(())
}

fn auth_header(api_key: &str) -> String {
    let payload = format!("API_KEY:{api_key}");
    format!("Basic {}", BASE64_STANDARD.encode(payload))
}
