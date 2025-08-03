use clap::Parser;
use base64::prelude::*;
use serde::Deserialize;
use std::env;

#[derive(Parser)]
struct Cli {
    /// Athelete id or activity id
    #[arg(short, long)]
    id: String,

    /// The command to run
    command: String,
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

    match args.command.as_str() {
        "list" => list_activities(&api_key, &args.id).await,
        "get" => get_activity(&api_key, &args.id).await,
        _ => println!("Unknown command: {}", args.command),
    }
}

const ENDPOINT: &str = "https://intervals.icu";

// id,start_date_local,name,type,moving_time,distance,elapsed_time,total_elevation_gain,max_speed,average_speed,has_heartrate,max_heartrate,average_heartrate,average_cadence,calories,device_watts,icu_average_watts,icu_normalized_watts,icu_joules,icu_intensity,icu_training_load,icu_training_load_edited,icu_rpe,pace,icu_fatigue,icu_fitness,icu_eftp,icu_variability,icu_efficiency,trainer,commute,race,sub_type,icu_ftp,icu_w_prime,threshold_pace,power_load,hr_load,pace_load,icu_resting_hr,lthr,hr_z1,hr_z2,hr_z3,hr_z4,hr_z5,hr_z6,hr_max,hr_z1_secs,hr_z2_secs,hr_z3_secs,hr_z4_secs,hr_z5_secs,hr_z6_secs,hr_z7_secs,z1_secs,z2_secs,z3_secs,z4_secs,z5_secs,z6_secs,z7_secs,sweet_spot_secs,icu_weight,icu_ignore_power,icu_ignore_hr,icu_ignore_time,icu_recording_time,icu_warmup_time,icu_cooldown_time,icu_hrrc,icu_hrrc_start_bpm,icu_power_spike_threshold,icu_pm_ftp,icu_pm_cp,icu_pm_w_prime,icu_pm_p_max,start_date,icu_sync_date,timezone,file_type,external_id,compliance,gear,description
#[derive(Debug, Deserialize)]
struct Activity {
    id: String,
    name: String,
    start_date_local: String,
    elapsed_time: i64,
    distance: Option<f64>,
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

fn auth_header(api_key: &str) -> String {
    let payload = format!("API_KEY:{api_key}");
    format!("Basic {}", BASE64_STANDARD.encode(payload))
}
