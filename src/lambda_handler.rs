use lambda_runtime::{Error, LambdaEvent};
use serde::{Deserialize, Serialize};
use std::env;
use std::path::PathBuf;
use aws_config::BehaviorVersion;
use aws_sdk_s3::Client as S3Client;
use aws_sdk_secretsmanager::Client as SecretsManagerClient;

use crate::sync::sync_activities;

#[derive(Deserialize)]
pub struct LambdaRequest {
    pub athlete_id: String,
}

#[derive(Serialize)]
pub struct LambdaResponse {
    pub message: String,
    pub s3_bucket: String,
    pub files_processed: u32,
}



pub async fn function_handler(event: LambdaEvent<LambdaRequest>) -> Result<LambdaResponse, Error> {
    let (event, _context) = event.into_parts();
    
    // Get required environment variables
    let secret_arn = env::var("SECRETS_MANAGER_SECRET_ARN")
        .map_err(|_| Error::from("SECRETS_MANAGER_SECRET_ARN environment variable not set"))?;
    
    let s3_bucket = env::var("S3_BUCKET")
        .map_err(|_| Error::from("S3_BUCKET environment variable not set"))?;
    
    // Initialize AWS SDK
    let config = aws_config::load_defaults(BehaviorVersion::latest()).await;
    let _s3_client = S3Client::new(&config);
    let secrets_client = SecretsManagerClient::new(&config);
    
    // Retrieve API key from Secrets Manager
    let secret_value = secrets_client
        .get_secret_value()
        .secret_id(&secret_arn)
        .send()
        .await
        .map_err(|e| Error::from(format!("Failed to retrieve secret: {e}")))?;
    
    let api_key = secret_value
        .secret_string()
        .ok_or_else(|| Error::from("Secret string not found"))?;
    
    // For now, use the existing sync_activities function with a temporary output directory
    // This will need to be adapted to write to S3 instead of local filesystem
    let temp_dir = PathBuf::from("/tmp");
    sync_activities(api_key, &event.athlete_id, &temp_dir).await;
    
    Ok(LambdaResponse {
        message: format!("Successfully synced activities for athlete {}", event.athlete_id),
        s3_bucket,
        files_processed: 0, // TODO: implement proper counting
    })
}