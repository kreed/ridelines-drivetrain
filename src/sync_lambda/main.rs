use lambda_runtime::{Error, LambdaEvent};
use metrics_cloudwatch_embedded::lambda::handler::run;
use serde::{Deserialize, Serialize};
use tracing::info_span;

use aws_config::BehaviorVersion;
use aws_sdk_dynamodb::Client as DynamoDBClient;
use aws_sdk_s3::Client as S3Client;
use function_timer::time;
use std::env;
use tempdir::TempDir;

use ridelines_drivetrain::common::{intervals_client::IntervalsClient, metrics, types::User};

#[derive(Debug, Deserialize, Serialize)]
pub struct SyncRequest {
    pub user_id: String,
}

mod activity_sync;
mod fit_converter;
mod tile_generator;

use crate::activity_sync::ActivitySync;
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
        .cloudwatch_namespace(metrics::METRICS_NAMESPACE)
        .lambda_cold_start_span(info_span!("cold start"))
        .lambda_cold_start_metric("ColdStart")
        .with_lambda_request_id("RequestId")
        .init()?;

    run(metrics, function_handler).await
}

#[time("lambda_handler_duration")]
pub(crate) async fn function_handler(event: LambdaEvent<SyncRequest>) -> Result<(), Error> {
    // Extract some useful information from the request
    let (payload, _context) = event.into_parts();
    tracing::info!("Sync request: {:?}", payload);

    let user_id = &payload.user_id;

    let s3_bucket =
        env::var("S3_BUCKET").map_err(|_| Error::from("S3_BUCKET environment variable not set"))?;
    let users_table = env::var("USERS_TABLE_NAME")
        .map_err(|_| Error::from("USERS_TABLE_NAME environment variable not set"))?;

    // Initialize AWS SDK
    let config = aws_config::load_defaults(BehaviorVersion::latest()).await;
    let s3_client = S3Client::new(&config);
    let dynamodb_client = DynamoDBClient::new(&config);

    // Load user data and access token from DynamoDB
    let user: User = dynamodb_client
        .get_item()
        .table_name(&users_table)
        .key(
            "id",
            aws_sdk_dynamodb::types::AttributeValue::S(user_id.clone()),
        )
        .send()
        .await
        .map_err(|e| Error::from(format!("Failed to get user from DynamoDB: {e}")))?
        .item()
        .ok_or_else(|| Error::from("User not found in DynamoDB"))
        .and_then(|item| {
            serde_dynamo::from_item(item.clone())
                .map_err(|e| Error::from(format!("Failed to deserialize user: {e}")))
        })?;

    // Create shared work directory for all temporary files
    let work_dir = TempDir::new(&format!("intervals_mapper_{}", user.athlete_id))
        .map_err(|e| Error::from(format!("Failed to create work directory: {e}")))?;

    // Create IntervalsClient with access token
    let mut intervals_client = IntervalsClient::new();
    intervals_client.set_access_token(&user.intervals_access_token);

    // Sync activities and get path to concatenated GeoJSON file
    let sync_job = ActivitySync::new(
        intervals_client,
        &user.athlete_id,
        s3_client.clone(),
        &s3_bucket,
        work_dir.path(),
    );

    let geojson_file_path = match sync_job.sync_activities().await {
        Ok(Some(path)) => path,
        Ok(None) => {
            // No changes detected, skip tile generation
            tracing::info!("No activity changes detected, Lambda execution completed successfully");
            metrics::increment_lambda_success();
            return Ok(());
        }
        Err(e) => {
            metrics::increment_lambda_failure();
            return Err(Error::from(format!("Sync failed: {e}")));
        }
    };

    // Generate PMTiles from the concatenated GeoJSON file
    let tile_generator = TileGenerator::new(s3_client, user.athlete_id.clone())
        .map_err(|e| Error::from(format!("Failed to create TileGenerator: {e}")))?;

    let tile_result = tile_generator
        .generate_pmtiles_from_file(&geojson_file_path.to_string_lossy())
        .await;

    // Clean up the GeoJSON file regardless of tile generation success/failure
    let _ = std::fs::remove_file(&geojson_file_path);

    if let Err(e) = tile_result {
        tracing::error!("Failed to generate PMTiles: {}", e);
        metrics::increment_lambda_failure();
        return Err(Error::from(format!("PMTiles generation failed: {e}")));
    }

    // Record successful Lambda execution
    metrics::increment_lambda_success();
    Ok(())
}
