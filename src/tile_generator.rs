use crate::activity_archive::ActivityArchiveManager;
use crate::metrics_helper;
use aws_sdk_s3::Client as S3Client;
use aws_sdk_s3::primitives::ByteStream;
use function_timer::time;
use std::process::Command;
use tokio::fs;
use tracing::{error, info};

pub struct TileGenerator {
    s3_client: S3Client,
    s3_bucket: String,
    athlete_id: String,
}

impl TileGenerator {
    pub fn new(s3_client: S3Client, s3_bucket: String, athlete_id: String) -> Self {
        Self {
            s3_client,
            s3_bucket,
            athlete_id,
        }
    }

    #[time("generate_pmtiles_duration")]
    pub async fn generate_pmtiles(&self) -> Result<(), Box<dyn std::error::Error>> {
        info!(
            "Starting PMTiles generation for athlete {}",
            self.athlete_id
        );

        // Create temporary files
        let temp_data_file = format!("/tmp/all-activities-{}.dat", self.athlete_id);
        let temp_pmtiles_file = format!("/tmp/{}.pmtiles", self.athlete_id);

        // Phase 1: List, download, and concatenate GeoJSON files (timed)
        self.prepare_geojson_data(&temp_data_file).await?;

        // Phase 2: Run tippecanoe to generate PMTiles (timed)
        self.run_tippecanoe(&temp_data_file, &temp_pmtiles_file)
            .await?;

        // Phase 3: Upload PMTiles to S3 (timed)
        self.upload_pmtiles(&temp_pmtiles_file).await?;

        // Clean up temp files
        let _ = fs::remove_file(&temp_data_file).await;
        let _ = fs::remove_file(&temp_pmtiles_file).await;

        Ok(())
    }

    #[time("prepare_geojson_data_duration")]
    async fn prepare_geojson_data(
        &self,
        temp_data_file: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
        // Load the existing activity archive
        let existing_archive = ActivityArchiveManager::load_existing(
            &self.s3_client,
            &self.s3_bucket,
            &self.athlete_id,
        )
        .await?;

        // Extract all GeoJSON data from the archive
        let geojson_entries = existing_archive.extract_geojson_data();

        if geojson_entries.is_empty() {
            return Err("No GeoJSON data found in archive".into());
        }

        info!("Found {} GeoJSON entries in archive", geojson_entries.len());

        // Concatenate all GeoJSON entries
        let mut concatenated_content = String::new();
        for geojson_data in &geojson_entries {
            concatenated_content.push_str(geojson_data);
            concatenated_content.push('\n'); // Add newline between entries
        }

        // Write concatenated content to temp file
        fs::write(temp_data_file, &concatenated_content)
            .await
            .map_err(|e| format!("Failed to write temporary data file: {e}"))?;

        // Record concatenated file size
        metrics_helper::record_geojson_concatenated_size(concatenated_content.len() as u64);

        info!(
            "Successfully created temporary file with {} activities from archive",
            geojson_entries.len()
        );
        Ok(())
    }

    #[time("tippecanoe_execution_duration")]
    async fn run_tippecanoe(
        &self,
        input_file: &str,
        output_file: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
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
            .map_err(|e| format!("Failed to execute tippecanoe: {e}"))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
            error!("Tippecanoe failed with status: {}", output.status);
            error!("Stderr: {}", stderr);
            error!("Stdout: {}", stdout);
            metrics_helper::increment_tippecanoe_failure();
            return Err(format!("Tippecanoe failed: {stderr}").into());
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        if !stdout.is_empty() {
            info!("Tippecanoe output: {}", stdout);
        }

        metrics_helper::increment_tippecanoe_success();
        Ok(())
    }

    #[time("pmtiles_upload_duration")]
    async fn upload_pmtiles(&self, pmtiles_file: &str) -> Result<(), Box<dyn std::error::Error>> {
        info!("Uploading PMTiles file to S3: {}", pmtiles_file);

        // Read the PMTiles file
        let file_content = fs::read(pmtiles_file)
            .await
            .map_err(|e| format!("Failed to read PMTiles file: {e}"))?;

        // Record PMTiles file size
        metrics_helper::record_pmtiles_file_size(file_content.len() as u64);

        // Upload to website S3 bucket
        let s3_key = format!("strava/{}.pmtiles", self.athlete_id);

        match self
            .s3_client
            .put_object()
            .bucket("kreed.org-website")
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
                Err(format!("Failed to upload PMTiles to S3: {e}").into())
            }
        }
    }
}
