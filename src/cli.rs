use clap::{Parser, Subcommand};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use crate::intervals_client::IntervalsClient;
use crate::sync::sync_activities;

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
    /// Download FIT file for a specific activity
    Download {
        /// Activity ID
        #[arg(short, long)]
        id: String,
        /// Path to save the FIT file
        #[arg(short, long)]
        path: PathBuf,
    },
    /// Sync all activities as GeoJSON files for an athlete
    Sync {
        /// Athlete ID
        #[arg(short, long)]
        id: String,
        /// Output directory for GeoJSON files
        #[arg(short, long)]
        output_dir: PathBuf,
    },
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
            eprintln!("Error fetching activities: {e}");
        }
    }
}

async fn download_activity(
    api_key: &str,
    activity_id: &str,
    output_path: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let client = IntervalsClient::new(api_key.to_string());

    let fit_data = client
        .download_fit(activity_id)
        .await
        .inspect_err(|e| eprintln!("Error downloading FIT for activity {activity_id}: {e}"))?;

    fs::write(output_path, fit_data)
        .inspect_err(|e| eprintln!("Error writing FIT file to {}: {}", output_path.display(), e))?;

    println!("FIT file saved to: {}", output_path.display());
    Ok(())
}

pub async fn cli_main() {
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
        Commands::Download { id, path } => {
            if download_activity(&api_key, &id, &path).await.is_err() {
                std::process::exit(1);
            }
        }
        Commands::Sync { id, output_dir } => sync_activities(&api_key, &id, &output_dir).await,
    }
}