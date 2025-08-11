use clap::{Parser, Subcommand};
use std::env;
use std::path::PathBuf;

mod convert;
mod download;
mod intervals_client;
use download::{download_activity, download_all_activities, list_activities};

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
        Commands::Download { id, path } => {
            if download_activity(&api_key, &id, &path).await.is_err() {
                std::process::exit(1);
            }
        }
        Commands::DownloadAll { id, output_dir } => {
            download_all_activities(&api_key, &id, &output_dir).await
        }
    }
}
