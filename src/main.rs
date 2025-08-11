use clap::{Parser, Subcommand};
use std::env;
use std::path::PathBuf;

mod convert;
mod download;
mod intervals_client;
mod sync;
use download::{download_activity, list_activities};
use sync::sync_activities;

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
        Commands::Download { id, path } => {
            if download_activity(&api_key, &id, &path).await.is_err() {
                std::process::exit(1);
            }
        }
        Commands::Sync { id, output_dir } => sync_activities(&api_key, &id, &output_dir).await,
    }
}
