use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fmt;

// DynamoDB User table record
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct User {
    pub id: String,
    pub athlete_id: String,
    pub name: Option<String>,
    pub email: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub last_login: DateTime<Utc>,
}

#[derive(Debug)]
pub enum CommonError {
    Http(reqwest::StatusCode),
    Network(String),
    Io(std::io::Error),
    Serialization(serde_json::Error),
    Configuration(String),
    Authentication(String),
    Other(String),
}

impl fmt::Display for CommonError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CommonError::Http(status) => write!(f, "HTTP {status}"),
            CommonError::Network(msg) => write!(f, "Network error: {msg}"),
            CommonError::Io(err) => write!(f, "IO error: {err}"),
            CommonError::Serialization(err) => write!(f, "Serialization error: {err}"),
            CommonError::Configuration(msg) => write!(f, "Configuration error: {msg}"),
            CommonError::Authentication(msg) => write!(f, "Authentication error: {msg}"),
            CommonError::Other(msg) => write!(f, "{msg}"),
        }
    }
}

impl std::error::Error for CommonError {}

impl From<std::io::Error> for CommonError {
    fn from(err: std::io::Error) -> Self {
        CommonError::Io(err)
    }
}

impl From<serde_json::Error> for CommonError {
    fn from(err: serde_json::Error) -> Self {
        CommonError::Serialization(err)
    }
}

impl From<reqwest::Error> for CommonError {
    fn from(err: reqwest::Error) -> Self {
        CommonError::Network(err.to_string())
    }
}

impl From<anyhow::Error> for CommonError {
    fn from(err: anyhow::Error) -> Self {
        CommonError::Other(err.to_string())
    }
}

// Re-export commonly used types from intervals_client for convenience
pub use crate::common::intervals_client::{
    Activity, IntervalsUserProfile, OAuthTokenRequest, OAuthTokenResponse,
};

// Common result type alias
pub type CommonResult<T> = Result<T, CommonError>;
