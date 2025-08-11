use crate::intervals_client::IntervalsClient;
use std::fs;
use std::path::Path;

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

pub async fn download_activity(
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
