use crate::metrics_helper;
use anyhow::{Context, Result};
use aws_sdk_s3::Client as S3Client;
use aws_sdk_s3::primitives::ByteStream;
use function_timer::time;
use std::process::Command;
use tokio::fs;
use tracing::{error, info};

const WEBSITE_S3_BUCKET: &str = "kreed.org-website";

pub struct TileGenerator {
    s3_client: S3Client,
    athlete_id: String,
}

impl TileGenerator {
    pub fn new(s3_client: S3Client, athlete_id: String) -> Self {
        Self {
            s3_client,
            athlete_id,
        }
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

        // Upload to website S3 bucket
        let s3_key = format!("strava/{}.pmtiles", self.athlete_id);

        match self
            .s3_client
            .put_object()
            .bucket(WEBSITE_S3_BUCKET)
            .key(&s3_key)
            .body(ByteStream::from(file_content))
            .content_type("application/vnd.pmtiles")
            .send()
            .await
        {
            Ok(_) => {
                metrics_helper::increment_s3_upload_success();
                info!("Successfully uploaded PMTiles to website S3: {}", s3_key);
                Ok(())
            }
            Err(e) => {
                metrics_helper::increment_s3_upload_failure();
                Err(anyhow::anyhow!("Failed to upload PMTiles to S3: {e}"))
            }
        }
    }
}
