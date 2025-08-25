use crate::common::metrics;
use anyhow::Result;
use reqwest::StatusCode;
use reqwest_middleware::{ClientBuilder, ClientWithMiddleware};
use reqwest_retry::{RetryTransientMiddleware, policies::ExponentialBackoff};
use serde::{Deserialize, Serialize};
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

// OAuth types
#[derive(Serialize)]
pub struct OAuthTokenRequest {
    pub grant_type: String, // "authorization_code"
    pub code: String,
    pub redirect_uri: String,
    pub client_id: String,
    pub client_secret: String,
}

#[derive(Deserialize)]
pub struct OAuthTokenResponse {
    pub access_token: String,
    pub token_type: String,
    pub expires_in: Option<i32>,
    pub refresh_token: Option<String>,
}

#[derive(Deserialize)]
pub struct IntervalsUserProfile {
    pub id: String,
    pub name: String,
    pub username: Option<String>,
    pub email: Option<String>,
    pub avatar: Option<String>,
}

pub struct IntervalsClient {
    client: ClientWithMiddleware,
    auth_header: Option<String>,
}

impl Default for IntervalsClient {
    fn default() -> Self {
        Self::new()
    }
}

impl IntervalsClient {
    pub fn new() -> Self {
        // Create client with retry middleware for OAuth flows
        let retry_policy = ExponentialBackoff::builder().build_with_max_retries(2);
        let client = ClientBuilder::new(reqwest::Client::new())
            .with(RetryTransientMiddleware::new_with_policy(retry_policy))
            .build();

        Self {
            client,
            auth_header: None,
        }
    }

    pub fn set_access_token(&mut self, access_token: &str) {
        self.auth_header = Some(format!("Bearer {access_token}"));
    }

    pub async fn fetch_activities(&self, athlete_id: &str) -> Result<Vec<Activity>> {
        let path = format!("{ENDPOINT}/api/v1/athlete/{athlete_id}/activities.csv");

        let auth_header = self
            .auth_header
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("No access token set"))?;

        match self
            .client
            .get(path)
            .header("Authorization", auth_header)
            .send()
            .await
        {
            Ok(response) => match response.text().await {
                Ok(body) => {
                    metrics::increment_intervals_api_success();

                    let mut rdr = csv::Reader::from_reader(body.as_bytes());
                    let mut activities = Vec::new();

                    for result in rdr.deserialize() {
                        let activity: Activity = result?;
                        activities.push(activity);
                    }

                    Ok(activities)
                }
                Err(e) => {
                    metrics::increment_intervals_api_failure();
                    Err(e.into())
                }
            },
            Err(e) => {
                metrics::increment_intervals_api_failure();
                Err(e.into())
            }
        }
    }

    pub async fn download_fit(&self, activity_id: &str) -> Result<Option<Vec<u8>>, DownloadError> {
        let path = format!("{ENDPOINT}/api/v1/activity/{activity_id}/fit-file");

        let auth_header = self.auth_header.as_ref().ok_or_else(|| {
            DownloadError::Network(reqwest_middleware::Error::Middleware(anyhow::anyhow!(
                "No access token set"
            )))
        })?;

        let response = self
            .client
            .get(path)
            .header("Authorization", auth_header)
            .send()
            .await
            .map_err(|e| {
                metrics::increment_intervals_api_failure();
                DownloadError::Network(e)
            })?;

        let status = response.status();
        if !status.is_success() {
            if status.as_u16() == 422 {
                // HTTP 422 means no GPS data available - return None
                metrics::increment_intervals_api_success();
                return Ok(None);
            } else {
                metrics::increment_intervals_api_failure();
                return Err(DownloadError::Http(status));
            }
        }

        let body = response.bytes().await.map_err(|e| {
            metrics::increment_intervals_api_failure();
            DownloadError::Network(reqwest_middleware::Error::Reqwest(e))
        })?;

        metrics::increment_intervals_api_success();
        Ok(Some(body.to_vec()))
    }

    // OAuth methods
    pub async fn exchange_oauth_code(
        &self,
        request: OAuthTokenRequest,
    ) -> Result<OAuthTokenResponse> {
        let path = format!("{ENDPOINT}/api/oauth/token");

        let response = self
            .client
            .post(path)
            .form(&request)
            .send()
            .await
            .inspect_err(|_e| {
                metrics::increment_intervals_api_failure();
            })?;

        if !response.status().is_success() {
            metrics::increment_intervals_api_failure();
            return Err(anyhow::anyhow!(
                "OAuth token exchange failed with status: {}",
                response.status()
            ));
        }

        let response_text = response.text().await.inspect_err(|_e| {
            metrics::increment_intervals_api_failure();
        })?;

        let token_response: OAuthTokenResponse =
            serde_json::from_str(&response_text).map_err(|e| {
                metrics::increment_intervals_api_failure();
                anyhow::anyhow!("Failed to parse OAuth token response: {}", e)
            })?;

        metrics::increment_intervals_api_success();
        Ok(token_response)
    }

    pub async fn get_user_profile(&self) -> Result<IntervalsUserProfile> {
        let path = format!("{ENDPOINT}/api/athlete");

        let auth_header = self
            .auth_header
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("No access token set"))?;

        let response = self
            .client
            .get(path)
            .header("Authorization", auth_header)
            .send()
            .await
            .inspect_err(|_e| {
                metrics::increment_intervals_api_failure();
            })?;

        if !response.status().is_success() {
            metrics::increment_intervals_api_failure();
            return Err(anyhow::anyhow!(
                "Failed to get user profile with status: {}",
                response.status()
            ));
        }

        let response_text = response.text().await.inspect_err(|_e| {
            metrics::increment_intervals_api_failure();
        })?;

        let profile: IntervalsUserProfile = serde_json::from_str(&response_text).map_err(|e| {
            metrics::increment_intervals_api_failure();
            anyhow::anyhow!("Failed to parse user profile response: {}", e)
        })?;

        metrics::increment_intervals_api_success();
        Ok(profile)
    }
}
