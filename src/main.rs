use aws_lambda_events::event::eventbridge::EventBridgeEvent;
use lambda_runtime::{Error, LambdaEvent};
use metrics_cloudwatch_embedded::lambda::handler::run;
use tracing::info_span;

use aws_config::BehaviorVersion;
use aws_sdk_s3::Client as S3Client;
use aws_sdk_secretsmanager::Client as SecretsManagerClient;
use function_timer::time;
use std::env;

mod activity_archive;
mod activity_sync;
mod convert;
mod intervals_client;
mod metrics_helper;
mod tile_generator;

use crate::activity_sync::SyncJob;
use crate::tile_generator::TileGenerator;

#[tokio::main]
async fn main() -> Result<(), Error> {
    tracing_subscriber::fmt()
        .json()
        .with_env_filter(tracing_subscriber::filter::EnvFilter::from_default_env())
        .with_target(false)
        .with_current_span(false)
        .without_time()
        .init();

    let metrics = metrics_cloudwatch_embedded::Builder::new()
        .cloudwatch_namespace("IntervalsMapper")
        .lambda_cold_start_span(info_span!("cold start"))
        .lambda_cold_start_metric("ColdStart")
        .with_lambda_request_id("RequestId")
        .init()?;

    run(metrics, function_handler).await
}

/// This is the main body for the function.
/// Write your code inside it.
/// There are some code example in the following URLs:
/// - https://github.com/awslabs/aws-lambda-rust-runtime/tree/main/examples
/// - https://github.com/aws-samples/serverless-rust-demo/
#[time("lambda_handler_duration")]
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
    let sync_job = SyncJob::new(api_key, athlete_id, s3_client.clone(), &s3_bucket);

    if let Err(e) = sync_job.sync_activities().await {
        metrics_helper::increment_lambda_failure();
        return Err(Error::from(format!("Sync failed: {e}")));
    }

    // Generate PMTiles from synced GeoJSON files
    let tile_generator = TileGenerator::new(s3_client, athlete_id.to_string());

    if let Err(e) = tile_generator.generate_pmtiles().await {
        tracing::error!("Failed to generate PMTiles: {}", e);
        metrics_helper::increment_lambda_failure();
        return Err(Error::from(format!("PMTiles generation failed: {e}")));
    }

    // Record successful Lambda execution
    metrics_helper::increment_lambda_success();
    Ok(())
}
