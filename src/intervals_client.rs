use base64::prelude::*;
use reqwest_middleware::{ClientBuilder, ClientWithMiddleware};
use reqwest_retry::{RetryTransientMiddleware, policies::ExponentialBackoff};
use serde::Deserialize;
use std::path::Path;
use std::fs;
use reqwest::StatusCode;

const ENDPOINT: &str = "https://intervals.icu";

#[derive(Debug)]
pub enum DownloadError {
    Http(StatusCode),
    Network(reqwest_middleware::Error),
    Io(std::io::Error),
}

impl std::fmt::Display for DownloadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DownloadError::Http(status) => write!(f, "HTTP {}", status),
            DownloadError::Network(e) => write!(f, "Network error: {}", e),
            DownloadError::Io(e) => write!(f, "IO error: {}", e),
        }
    }
}

impl std::error::Error for DownloadError {}

#[derive(Debug, Deserialize, Clone)]
pub struct Activity {
    pub id: String,
    pub name: String,
    pub start_date_local: String,
    pub distance: Option<f64>,
    pub trainer: Option<bool>,
    #[serde(rename = "type")]
    pub activity_type: String,
    pub elapsed_time: i64,
}

pub struct IntervalsClient {
    client: ClientWithMiddleware,
    api_key: String,
}

impl IntervalsClient {
    pub fn new(api_key: String) -> Self {
        // Create client with retry middleware
        let retry_policy = ExponentialBackoff::builder().build_with_max_retries(2);
        let client = ClientBuilder::new(reqwest::Client::new())
            .with(RetryTransientMiddleware::new_with_policy(retry_policy))
            .build();

        Self { client, api_key }
    }

    pub async fn fetch_activities(&self, athlete_id: &str) -> Result<Vec<Activity>, Box<dyn std::error::Error>> {
        let path = format!("{ENDPOINT}/api/v1/athlete/{athlete_id}/activities.csv");
        let body = self.client.get(path)
            .header("Authorization", self.auth_header())
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

    pub async fn download_gpx(&self, activity_id: &str, file_path: &Path) -> Result<(), DownloadError> {
        let path = format!("{ENDPOINT}/api/v1/activity/{activity_id}/gpx-file");
        let response = self.client.get(path)
            .header("Authorization", self.auth_header())
            .send()
            .await
            .map_err(|e| DownloadError::Network(e))?;
        
        let status = response.status();
        if !status.is_success() {
            return Err(DownloadError::Http(status));
        }
        
        let body = response.text().await.map_err(|e| DownloadError::Network(reqwest_middleware::Error::Reqwest(e)))?;
        fs::write(file_path, body).map_err(|e| DownloadError::Io(e))?;
        
        Ok(())
    }

    fn auth_header(&self) -> String {
        let payload = format!("API_KEY:{}", self.api_key);
        format!("Basic {}", BASE64_STANDARD.encode(payload))
    }
}