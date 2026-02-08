use aws_lambda_events::event::sqs::SqsEvent;
use lambda_runtime::{Error, LambdaEvent};
use metrics_cloudwatch_embedded::lambda::handler::run;
use serde::{Deserialize, Serialize};
use tracing::info_span;

use aws_config::BehaviorVersion;
use aws_sdk_s3::Client as S3Client;
use function_timer::time;
use std::env;
use tempdir::TempDir;

use clerk_rs::apis::users_api::User as ClerkUser;
use clerk_rs::{ClerkConfiguration, clerk::Clerk};
use ridelines_drivetrain::common::{intervals_client::IntervalsClient, metrics};

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncRequest {
    pub user_id: String,
    pub sync_id: String,
    pub timestamp: String,
}

mod activity_sync;
mod fit_converter;
mod sync_status;
mod tile_generator;

use crate::activity_sync::ActivitySync;
use crate::tile_generator::TileGenerator;
use std::sync::Arc;

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
pub(crate) async fn function_handler(event: LambdaEvent<SqsEvent>) -> Result<(), Error> {
    // Extract some useful information from the request
    let (sqs_event, _context) = event.into_parts();
    tracing::info!(
        "Received SQS event with {} records",
        sqs_event.records.len()
    );

    // Process each record (should only be 1 based on our batch_size configuration)
    for record in sqs_event.records {
        // Parse the message body to get the sync request
        let body = record
            .body
            .as_ref()
            .ok_or_else(|| Error::from("SQS message body is empty"))?;
        let sync_request: SyncRequest = serde_json::from_str(body)
            .map_err(|e| Error::from(format!("Failed to parse SQS message body: {e}")))?;

        tracing::info!(
            "Processing sync request for user: {} sync: {}",
            sync_request.user_id,
            sync_request.sync_id
        );

        // Process the sync for this user
        process_user_sync(&sync_request.user_id, &sync_request.sync_id).await?;
    }

    Ok(())
}

async fn process_user_sync(user_id: &str, sync_id: &str) -> Result<(), Error> {
    let s3_bucket =
        env::var("S3_BUCKET").map_err(|_| Error::from("S3_BUCKET environment variable not set"))?;

    // Initialize AWS SDK
    let config = aws_config::load_defaults(BehaviorVersion::latest()).await;
    let s3_client = S3Client::new(&config);
    let dynamodb_client = aws_sdk_dynamodb::Client::new(&config);

    // Initialize sync status updater
    let sync_status = Arc::new(sync_status::SyncStatusUpdater::new(
        dynamodb_client.clone(),
        user_id.to_string(),
        sync_id.to_string(),
    ));
    sync_status.initialize().await?;

    // Create shared work directory for all temporary files
    let work_dir = TempDir::new(&format!("intervals_mapper_{}", user_id))
        .map_err(|e| Error::from(format!("Failed to create work directory: {e}")))?;

    // Get intervals.icu access token from Clerk
    let access_token = get_intervals_access_token_from_clerk(user_id).await?;

    // Create IntervalsClient with access token
    let mut intervals_client = IntervalsClient::new();
    intervals_client.set_access_token(&access_token);

    // Sync activities and get path to concatenated GeoJSON file
    let sync_job = ActivitySync::new(
        intervals_client,
        user_id,
        s3_client.clone(),
        &s3_bucket,
        work_dir.path(),
        sync_status.clone(),
    );

    let geojson_file_path = match sync_job.sync_activities().await {
        Ok(Some(path)) => path,
        Ok(None) => {
            // No changes detected, skip tile generation
            tracing::info!("No activity changes detected, Lambda execution completed successfully");
            sync_status.mark_completed().await?;
            metrics::increment_lambda_success();
            return Ok(());
        }
        Err(e) => {
            let _ = sync_status
                .mark_failed(&format!("Activity sync failed: {e}"))
                .await;
            metrics::increment_lambda_failure();
            return Err(Error::from(format!("Sync failed: {e}")));
        }
    };

    // Update status: start generating tiles
    sync_status.start_generating();

    // Generate PMTiles from the concatenated GeoJSON file
    let tile_generator = TileGenerator::new(s3_client, dynamodb_client, user_id.to_string())
        .map_err(|e| Error::from(format!("Failed to create TileGenerator: {e}")))?;

    let tile_result = tile_generator
        .generate_pmtiles_from_file(&geojson_file_path.to_string_lossy())
        .await;

    // Clean up the GeoJSON file regardless of tile generation success/failure
    let _ = std::fs::remove_file(&geojson_file_path);

    match tile_result {
        Ok(()) => {
            // Update status: complete generating
            sync_status.complete_generating();
            sync_status.mark_completed().await?;

            // Record successful Lambda execution
            metrics::increment_lambda_success();
            Ok(())
        }
        Err(e) => {
            tracing::error!("Failed to generate PMTiles: {}", e);
            let _ = sync_status
                .mark_failed(&format!("PMTiles generation failed: {e}"))
                .await;
            metrics::increment_lambda_failure();
            Err(Error::from(format!("PMTiles generation failed: {e}")))
        }
    }
}

async fn get_intervals_access_token_from_clerk(user_id: &str) -> Result<String, Error> {
    let clerk_secret_key = env::var("CLERK_SECRET_KEY")
        .map_err(|_| Error::from("CLERK_SECRET_KEY environment variable not set"))?;

    let config = ClerkConfiguration::new(None, None, Some(clerk_secret_key), None);
    let clerk = Clerk::new(config);

    // Get OAuth access token for intervals.icu using Clerk API
    let oauth_tokens =
        ClerkUser::get_o_auth_access_token(&clerk, user_id, "oauth_custom_intervals_icu")
            .await
            .map_err(|e| {
                Error::from(format!("Failed to get intervals.icu token from Clerk: {e}"))
            })?;

    let access_token = oauth_tokens
        .first()
        .ok_or_else(|| Error::from("No intervals.icu token found in Clerk"))?
        .token
        .as_ref()
        .ok_or_else(|| Error::from("Token field is empty"))?
        .clone();

    Ok(access_token)
}
