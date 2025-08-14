use aws_lambda_events::event::eventbridge::EventBridgeEvent;
use lambda_runtime::{Error, LambdaEvent, run, service_fn, tracing};

use aws_config::BehaviorVersion;
use aws_sdk_s3::Client as S3Client;
use aws_sdk_secretsmanager::Client as SecretsManagerClient;
use std::env;

mod convert;
mod intervals_client;
mod sync;

use crate::sync::SyncJob;

#[tokio::main]
async fn main() -> Result<(), Error> {
    tracing::init_default_subscriber();

    run(service_fn(function_handler)).await
}

/// This is the main body for the function.
/// Write your code inside it.
/// There are some code example in the following URLs:
/// - https://github.com/awslabs/aws-lambda-rust-runtime/tree/main/examples
/// - https://github.com/aws-samples/serverless-rust-demo/
pub(crate) async fn function_handler(event: LambdaEvent<EventBridgeEvent>) -> Result<(), Error> {
    // Extract some useful information from the request
    let (payload, _context) = event.into_parts();
    tracing::info!("Payload: {:?}", payload);

    // Extract athlete_id from the event detail
    // For now, we'll assume the athlete_id is passed in the detail field
    let athlete_id = payload
        .detail
        .get("athlete_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| Error::from("athlete_id not found in event detail"))?;

    // Get required environment variables
    let secret_arn = env::var("SECRETS_MANAGER_SECRET_ARN")
        .map_err(|_| Error::from("SECRETS_MANAGER_SECRET_ARN environment variable not set"))?;

    let s3_bucket =
        env::var("S3_BUCKET").map_err(|_| Error::from("S3_BUCKET environment variable not set"))?;
    
    // Initialize AWS SDK
    let config = aws_config::load_defaults(BehaviorVersion::latest()).await;
    let s3_client = S3Client::new(&config);
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

    // Sync activities directly to S3
    let sync_job = SyncJob::new(api_key, athlete_id, s3_client, &s3_bucket);
    
    if let Err(e) = sync_job.sync_activities().await {
        return Err(Error::from(format!("Sync failed: {e}")));
    }

    Ok(())
}
