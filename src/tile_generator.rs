use crate::metrics_helper;
use anyhow::{Context, Result};
use aws_sdk_cloudfront::Client as CloudFrontClient;
use aws_sdk_s3::Client as S3Client;
use aws_sdk_s3::primitives::ByteStream;
use chrono::Utc;
use function_timer::time;
use std::env;
use std::process::Command;
use tokio::fs;
use tracing::{error, info};

pub struct TileGenerator {
    s3_client: S3Client,
    cloudfront_client: CloudFrontClient,
    athlete_id: String,
    activities_bucket: String,
    cloudfront_distribution_id: String,
}

impl TileGenerator {
    pub fn new(
        s3_client: S3Client,
        cloudfront_client: CloudFrontClient,
        athlete_id: String,
    ) -> Result<Self> {
        let activities_bucket = env::var("ACTIVITIES_S3_BUCKET")
            .context("ACTIVITIES_S3_BUCKET environment variable not set")?;
        let cloudfront_distribution_id = env::var("CLOUDFRONT_DISTRIBUTION_ID")
            .context("CLOUDFRONT_DISTRIBUTION_ID environment variable not set")?;

        Ok(Self {
            s3_client,
            cloudfront_client,
            athlete_id,
            activities_bucket,
            cloudfront_distribution_id,
        })
    }

    #[time("generate_pmtiles_duration")]
    pub async fn generate_pmtiles_from_file(&self, geojson_file_path: &str) -> Result<()> {
        info!(
            "Starting PMTiles generation for athlete {} from file: {}",
            self.athlete_id, geojson_file_path
        );

        // Create temporary PMTiles file
        let temp_pmtiles_file = format!("/tmp/{}.pmtiles", self.athlete_id);

        // Phase 1: Run tippecanoe directly on the provided GeoJSON file
        self.run_tippecanoe(geojson_file_path, &temp_pmtiles_file)
            .await?;

        // Phase 2: Upload PMTiles to S3 (timed)
        self.upload_pmtiles(&temp_pmtiles_file).await?;

        // Phase 3: Invalidate CloudFront cache
        self.invalidate_cloudfront_cache().await?;

        // Clean up temp files
        let _ = fs::remove_file(&temp_pmtiles_file).await;

        Ok(())
    }

    #[time("tippecanoe_execution_duration")]
    async fn run_tippecanoe(&self, input_file: &str, output_file: &str) -> Result<()> {
        info!("Running tippecanoe: {} -> {}", input_file, output_file);

        let output = Command::new("/opt/bin/tippecanoe")
            .args([
                "--preserve-input-order",
                "-fl",
                "activities",
                "-o",
                output_file,
                input_file,
            ])
            .output()
            .context("Failed to execute tippecanoe")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
            error!("Tippecanoe failed with status: {}", output.status);
            error!("Stderr: {}", stderr);
            error!("Stdout: {}", stdout);
            metrics_helper::increment_tippecanoe_failure();
            return Err(anyhow::anyhow!("Tippecanoe failed: {stderr}"));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        if !stdout.is_empty() {
            info!("Tippecanoe output: {}", stdout);
        }

        metrics_helper::increment_tippecanoe_success();
        Ok(())
    }

    #[time("pmtiles_upload_duration")]
    async fn upload_pmtiles(&self, pmtiles_file: &str) -> Result<()> {
        info!("Uploading PMTiles file to S3: {}", pmtiles_file);

        // Read the PMTiles file
        let file_content = fs::read(pmtiles_file)
            .await
            .context("Failed to read PMTiles file")?;

        // Record PMTiles file size
        metrics_helper::record_pmtiles_file_size(file_content.len() as u64);

        // Upload to activities S3 bucket with new path structure
        let s3_key = format!("activities/{}.pmtiles", self.athlete_id);

        match self
            .s3_client
            .put_object()
            .bucket(&self.activities_bucket)
            .key(&s3_key)
            .body(ByteStream::from(file_content))
            .content_type("application/vnd.pmtiles")
            .send()
            .await
        {
            Ok(_) => {
                metrics_helper::increment_s3_upload_success();
                info!("Successfully uploaded PMTiles to activities S3: {}", s3_key);
                Ok(())
            }
            Err(e) => {
                metrics_helper::increment_s3_upload_failure();
                Err(anyhow::anyhow!("Failed to upload PMTiles to S3: {e}"))
            }
        }
    }

    #[time("cloudfront_invalidation_duration")]
    async fn invalidate_cloudfront_cache(&self) -> Result<()> {
        info!(
            "Invalidating CloudFront cache for athlete {}",
            self.athlete_id
        );

        let invalidation_path = format!("/activities/{}.pmtiles", self.athlete_id);

        match self
            .cloudfront_client
            .create_invalidation()
            .distribution_id(&self.cloudfront_distribution_id)
            .invalidation_batch(
                aws_sdk_cloudfront::types::InvalidationBatch::builder()
                    .paths(
                        aws_sdk_cloudfront::types::Paths::builder()
                            .quantity(1)
                            .items(invalidation_path.clone())
                            .build()
                            .context("Failed to build invalidation paths")?,
                    )
                    .caller_reference(format!("{}-{}", self.athlete_id, Utc::now().timestamp()))
                    .build()
                    .context("Failed to build invalidation batch")?,
            )
            .send()
            .await
        {
            Ok(response) => {
                info!(
                    "Successfully created CloudFront invalidation: {:?}",
                    response.invalidation().unwrap().id()
                );
                Ok(())
            }
            Err(e) => {
                error!("Failed to create CloudFront invalidation: {}", e);
                Err(anyhow::anyhow!(
                    "Failed to invalidate CloudFront cache: {e}"
                ))
            }
        }
    }
}
