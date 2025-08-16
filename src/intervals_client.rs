use crate::metrics_helper;
use anyhow::Result;
use base64::prelude::*;
use reqwest::StatusCode;
use reqwest_middleware::{ClientBuilder, ClientWithMiddleware};
use reqwest_retry::{RetryTransientMiddleware, policies::ExponentialBackoff};
use serde::Deserialize;
use std::hash::{Hash, Hasher};

const ENDPOINT: &str = "https://intervals.icu";

#[derive(Debug)]
pub enum DownloadError {
    Http(StatusCode),
    Network(reqwest_middleware::Error),
}

impl std::fmt::Display for DownloadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DownloadError::Http(status) => write!(f, "HTTP {status}"),
            DownloadError::Network(e) => write!(f, "Network error: {e}"),
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
    #[serde(rename = "type")]
    pub activity_type: String,
    pub elapsed_time: i64,
}

impl Hash for Activity {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.id.hash(state);
        self.name.hash(state);
        self.start_date_local.hash(state);
        self.elapsed_time.hash(state);
        if let Some(distance) = self.distance {
            distance.to_bits().hash(state);
        }
    }
}

impl Activity {
    pub fn compute_hash(&self) -> String {
        use std::collections::hash_map::DefaultHasher;
        
        let mut hasher = DefaultHasher::new();
        self.hash(&mut hasher);
        format!("{:x}", hasher.finish())
    }
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

    pub async fn fetch_activities(&self, athlete_id: &str) -> Result<Vec<Activity>> {
        let path = format!("{ENDPOINT}/api/v1/athlete/{athlete_id}/activities.csv");

        match self
            .client
            .get(path)
            .header("Authorization", self.auth_header())
            .send()
            .await
        {
            Ok(response) => match response.text().await {
                Ok(body) => {
                    metrics_helper::increment_intervals_api_success();

                    let mut rdr = csv::Reader::from_reader(body.as_bytes());
                    let mut activities = Vec::new();

                    for result in rdr.deserialize() {
                        let activity: Activity = result?;
                        activities.push(activity);
                    }

                    Ok(activities)
                }
                Err(e) => {
                    metrics_helper::increment_intervals_api_failure();
                    Err(e.into())
                }
            },
            Err(e) => {
                metrics_helper::increment_intervals_api_failure();
                Err(e.into())
            }
        }
    }

    pub async fn download_fit(&self, activity_id: &str) -> Result<Option<Vec<u8>>, DownloadError> {
        let path = format!("{ENDPOINT}/api/v1/activity/{activity_id}/fit-file");
        let response = self
            .client
            .get(path)
            .header("Authorization", self.auth_header())
            .send()
            .await
            .map_err(|e| {
                metrics_helper::increment_intervals_api_failure();
                DownloadError::Network(e)
            })?;

        let status = response.status();
        if !status.is_success() {
            if status.as_u16() == 422 {
                // HTTP 422 means no GPS data available - return None
                metrics_helper::increment_intervals_api_success();
                return Ok(None);
            } else {
                metrics_helper::increment_intervals_api_failure();
                return Err(DownloadError::Http(status));
            }
        }

        let body = response.bytes().await.map_err(|e| {
            metrics_helper::increment_intervals_api_failure();
            DownloadError::Network(reqwest_middleware::Error::Reqwest(e))
        })?;

        metrics_helper::increment_intervals_api_success();
        Ok(Some(body.to_vec()))
    }

    fn auth_header(&self) -> String {
        let payload = format!("API_KEY:{}", self.api_key);
        format!("Basic {}", BASE64_STANDARD.encode(payload))
    }
}
