use lambda_runtime::{Error, LambdaEvent};
use metrics_cloudwatch_embedded::lambda::handler::run;
use serde::{Deserialize, Serialize};
use tracing::info_span;

use aws_config::BehaviorVersion;
use aws_sdk_s3::Client as S3Client;
use function_timer::time;
use std::env;
use tempdir::TempDir;

use ridelines_drivetrain::common::metrics;

#[derive(Debug, Deserialize, Serialize)]
pub struct SyncRequest {
    pub athlete_id: String,
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

    let athlete_id = &payload.athlete_id;

    let s3_bucket =
        env::var("S3_BUCKET").map_err(|_| Error::from("S3_BUCKET environment variable not set"))?;

    // Initialize AWS SDK
    let config = aws_config::load_defaults(BehaviorVersion::latest()).await;
    let s3_client = S3Client::new(&config);

    // Create shared work directory for all temporary files
    let work_dir = TempDir::new(&format!("intervals_mapper_{athlete_id}"))
        .map_err(|e| Error::from(format!("Failed to create work directory: {e}")))?;

    // Sync activities and get path to concatenated GeoJSON file
    let sync_job = ActivitySync::new(athlete_id, s3_client.clone(), &s3_bucket, work_dir.path());

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
    let tile_generator = TileGenerator::new(s3_client, athlete_id.to_string())
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
