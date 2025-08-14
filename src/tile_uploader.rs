use aws_sdk_s3::Client as S3Client;
use aws_sdk_s3::primitives::ByteStream;
use std::boxed::Box;
use std::pin::Pin;
use std::process::Command;
use tokio::fs;
use tracing::{info, error};

type AsyncResult<'a> = Pin<Box<dyn std::future::Future<Output = Result<(), Box<dyn std::error::Error>>> + 'a>>;

pub struct TileUploader {
    s3_client: S3Client,
    athlete_id: String,
}

impl TileUploader {
    pub fn new(s3_client: S3Client, athlete_id: String) -> Self {
        Self {
            s3_client,
            athlete_id,
        }
    }

    pub async fn extract_and_upload_tiles(&self, mbtiles_file: &str, temp_tiles_dir: &str) -> Result<(), Box<dyn std::error::Error>> {
        info!("Extracting tiles from MBTiles using mb-util: {} -> {}", mbtiles_file, temp_tiles_dir);
        
        // Run mb-util to extract tiles
        let output = Command::new("python3")
            .args([
                "-m", "mbutil",
                "--image_format=pbf",
                mbtiles_file,
                temp_tiles_dir
            ])
            .output()
            .map_err(|e| format!("Failed to execute mb-util: {e}"))?;
        
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
            error!("mb-util failed with status: {}", output.status);
            error!("Stderr: {}", stderr);
            error!("Stdout: {}", stdout);
            return Err(format!("mb-util failed: {stderr}").into());
        }
        
        let stdout = String::from_utf8_lossy(&output.stdout);
        if !stdout.is_empty() {
            info!("mb-util output: {}", stdout);
        }
        
        // Upload all files in temp_tiles_dir to S3
        self.upload_tiles_directory(temp_tiles_dir).await?;
        
        // Clean up extracted tiles directory
        let _ = fs::remove_dir_all(temp_tiles_dir).await;
        
        Ok(())
    }

    async fn upload_tiles_directory(&self, tiles_dir: &str) -> Result<(), Box<dyn std::error::Error>> {
        info!("Uploading tiles directory to s3://kreed.org-website/strava/{}", self.athlete_id);
        
        // Walk through all files in the directory
        self.upload_directory_recursive(tiles_dir, tiles_dir, "kreed.org-website").await?;
        
        Ok(())
    }

    fn upload_directory_recursive<'a>(
        &'a self,
        base_dir: &'a str,
        current_dir: &'a str,
        bucket: &'a str,
    ) -> AsyncResult<'a> {
        Box::pin(async move {
        let mut dir = fs::read_dir(current_dir).await?;
        
        while let Some(entry) = dir.next_entry().await? {
            let path = entry.path();
            let file_name = path.to_string_lossy();
            
            if path.is_dir() {
                // Recursively upload subdirectories
                self.upload_directory_recursive(base_dir, &file_name, bucket).await?;
            } else {
                // Upload individual file
                let relative_path = path.strip_prefix(base_dir)
                    .map_err(|e| format!("Failed to get relative path: {e}"))?
                    .to_string_lossy();
                
                let s3_key = format!("strava/{}/{}", self.athlete_id, relative_path);
                
                // Read file content
                let file_content = fs::read(&path).await
                    .map_err(|e| format!("Failed to read file {file_name}: {e}"))?;
                
                // Determine content type based on file extension
                let content_type = if relative_path.ends_with(".pbf") {
                    "application/x-protobuf"
                } else if relative_path.ends_with(".json") {
                    "application/json"
                } else {
                    "application/octet-stream"
                };
                
                // Upload with gzip content-encoding
                self.s3_client
                    .put_object()
                    .bucket(bucket)
                    .key(&s3_key)
                    .body(ByteStream::from(file_content))
                    .content_type(content_type)
                    .content_encoding("gzip")
                    .send()
                    .await
                    .map_err(|e| format!("Failed to upload tile file {s3_key}: {e}"))?;
                
                info!("Uploaded tile: {}", s3_key);
            }
        }
        
        Ok(())
        })
    }
}