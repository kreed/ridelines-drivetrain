use aws_sdk_s3::Client as S3Client;
use aws_sdk_s3::primitives::ByteStream;
use function_timer::time;
use std::process::Command;
use tokio::fs;
use tracing::{info, warn, error};

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

    #[time("generate_mbtiles_duration")]
    pub async fn generate_mbtiles(&self) -> Result<String, Box<dyn std::error::Error>> {
        info!("Starting MBTiles generation for athlete {}", self.athlete_id);
        
        // Create temporary files
        let temp_data_file = format!("/tmp/all-activities-{}.dat", self.athlete_id);
        let temp_mbtiles_file = format!("/tmp/{}.mbtiles", self.athlete_id);
        
        // Phase 1: List, download, and concatenate GeoJSON files (timed)
        self.prepare_geojson_data(&temp_data_file).await?;
        
        // Phase 2: Run tippecanoe to generate MBTiles (timed)
        self.run_tippecanoe(&temp_data_file, &temp_mbtiles_file).await?;
        
        // Phase 3: Upload MBTiles to S3 (timed)
        self.upload_mbtiles(&temp_mbtiles_file).await?;
        
        // Clean up temp data file
        let _ = fs::remove_file(&temp_data_file).await;
        
        // Return path to temp mbtiles file for further processing
        Ok(temp_mbtiles_file)
    }

    #[time("prepare_geojson_data_duration")]
    async fn prepare_geojson_data(&self, temp_data_file: &str) -> Result<(), Box<dyn std::error::Error>> {
        // Get all .geojson files
        let geojson_files = self.list_geojson_files().await?;
        
        if geojson_files.is_empty() {
            return Err("No GeoJSON files found to process".into());
        }
        
        info!("Found {} GeoJSON files to process", geojson_files.len());
        
        // Concatenate all file contents to temp file
        self.concatenate_geojson_files(&geojson_files, temp_data_file).await?;
        
        Ok(())
    }

    async fn list_geojson_files(&self) -> Result<Vec<String>, Box<dyn std::error::Error>> {
        let s3_prefix = format!("athletes/{}", self.athlete_id);
        
        let files = self.s3_client
            .list_objects_v2()
            .bucket(&self.s3_bucket)
            .prefix(&s3_prefix)
            .into_paginator()
            .send()
            .try_collect()
            .await?
            .into_iter()
            .flat_map(|output| output.contents.unwrap_or_default())
            .filter_map(|object| object.key)
            .filter(|key| key.ends_with(".geojson"))
            .collect();
        
        Ok(files)
    }

    async fn concatenate_geojson_files(&self, geojson_files: &[String], temp_data_file: &str) -> Result<(), Box<dyn std::error::Error>> {
        let mut concatenated_content = String::new();
        let mut successful_files = 0;
        
        for file_key in geojson_files {
            match self.get_file_content(file_key).await {
                Ok(content) => {
                    concatenated_content.push_str(&content);
                    concatenated_content.push('\n'); // Add newline between files
                    successful_files += 1;
                }
                Err(e) => {
                    warn!("Failed to read file {} for concatenation: {}", file_key, e);
                }
            }
        }
        
        if successful_files == 0 {
            return Err("No files could be read for processing".into());
        }
        
        // Write concatenated content to temp file
        fs::write(temp_data_file, concatenated_content).await
            .map_err(|e| format!("Failed to write temporary data file: {e}"))?;
        
        info!("Successfully created temporary file with {} activities", successful_files);
        Ok(())
    }

    async fn get_file_content(&self, key: &str) -> Result<String, Box<dyn std::error::Error>> {
        let response = self.s3_client
            .get_object()
            .bucket(&self.s3_bucket)
            .key(key)
            .send()
            .await?;
            
        let body = response.body.collect().await?;
        let content = String::from_utf8(body.to_vec())?;
        Ok(content)
    }

    #[time("tippecanoe_execution_duration")]
    async fn run_tippecanoe(&self, input_file: &str, output_file: &str) -> Result<(), Box<dyn std::error::Error>> {
        info!("Running tippecanoe: {} -> {}", input_file, output_file);
        
        let output = Command::new("/opt/bin/tippecanoe")
            .args([
                "--preserve-input-order",
                "-fl", "activities",
                "-o", output_file,
                input_file
            ])
            .output()
            .map_err(|e| format!("Failed to execute tippecanoe: {e}"))?;
        
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
            error!("Tippecanoe failed with status: {}", output.status);
            error!("Stderr: {}", stderr);
            error!("Stdout: {}", stdout);
            return Err(format!("Tippecanoe failed: {stderr}").into());
        }
        
        let stdout = String::from_utf8_lossy(&output.stdout);
        if !stdout.is_empty() {
            info!("Tippecanoe output: {}", stdout);
        }
        
        Ok(())
    }

    #[time("mbtiles_upload_duration")]
    async fn upload_mbtiles(&self, mbtiles_file: &str) -> Result<(), Box<dyn std::error::Error>> {
        info!("Uploading MBTiles file to S3: {}", mbtiles_file);
        
        // Read the MBTiles file
        let file_content = fs::read(mbtiles_file).await
            .map_err(|e| format!("Failed to read MBTiles file: {e}"))?;
        
        // Upload to S3
        let s3_key = format!("athletes/{}.mbtiles", self.athlete_id);
        
        self.s3_client
            .put_object()
            .bucket(&self.s3_bucket)
            .key(&s3_key)
            .body(ByteStream::from(file_content))
            .content_type("application/vnd.mapbox-vector-tile")
            .send()
            .await
            .map_err(|e| format!("Failed to upload MBTiles to S3: {e}"))?;
        
        info!("Successfully uploaded MBTiles to S3: {}", s3_key);
        Ok(())
    }
}
