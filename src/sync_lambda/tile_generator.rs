use anyhow::{Context, Result};
use aws_sdk_dynamodb::Client as DynamoDbClient;
use aws_sdk_dynamodb::types::AttributeValue;
use aws_sdk_s3::Client as S3Client;
use aws_sdk_s3::primitives::ByteStream;
use function_timer::time;
use ridelines_drivetrain::common::metrics;
use sha2::{Digest, Sha256};
use std::env;
use std::process::Command;
use tokio::fs;
use tracing::{error, info};

pub struct TileGenerator {
    s3_client: S3Client,
    dynamodb_client: DynamoDbClient,
    user_id: String,
    activities_bucket: String,
    users_table_name: String,
}

impl TileGenerator {
    pub fn new(
        s3_client: S3Client,
        dynamodb_client: DynamoDbClient,
        user_id: String,
    ) -> Result<Self> {
        let activities_bucket = env::var("ACTIVITIES_S3_BUCKET")
            .context("ACTIVITIES_S3_BUCKET environment variable not set")?;
        let users_table_name = env::var("USERS_TABLE_NAME")
            .context("USERS_TABLE_NAME environment variable not set")?;

        Ok(Self {
            s3_client,
            dynamodb_client,
            user_id,
            activities_bucket,
            users_table_name,
        })
    }

    #[time("generate_pmtiles_duration")]
    pub async fn generate_pmtiles_from_file(&self, geojson_file_path: &str) -> Result<()> {
        info!(
            "Starting PMTiles generation for user {} from file: {}",
            self.user_id, geojson_file_path
        );

        // Create temporary PMTiles file
        let temp_pmtiles_file = format!("/tmp/{}.pmtiles", self.user_id);

        // Phase 1: Run tippecanoe directly on the provided GeoJSON file
        self.run_tippecanoe(geojson_file_path, &temp_pmtiles_file)
            .await?;

        // Phase 2: Upload PMTiles to S3 and update DynamoDB (timed)
        self.upload_pmtiles(&temp_pmtiles_file).await?;

        // Clean up temp files
        let _ = fs::remove_file(&temp_pmtiles_file).await;

        Ok(())
    }

    #[time("tippecanoe_execution_duration")]
    async fn run_tippecanoe(&self, input_file: &str, output_file: &str) -> Result<()> {
        info!("Running tippecanoe: {input_file} -> {output_file}");

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
            error!("Stderr: {stderr}");
            error!("Stdout: {stdout}");
            metrics::increment_tippecanoe_failure();
            return Err(anyhow::anyhow!("Tippecanoe failed: {stderr}"));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        if !stdout.is_empty() {
            info!("Tippecanoe output: {stdout}");
        }

        metrics::increment_tippecanoe_success();
        Ok(())
    }

    #[time("pmtiles_upload_duration")]
    async fn upload_pmtiles(&self, pmtiles_file: &str) -> Result<()> {
        info!("Uploading PMTiles file to S3: {pmtiles_file}");

        // Read the PMTiles file
        let file_content = fs::read(pmtiles_file)
            .await
            .context("Failed to read PMTiles file")?;

        // Record PMTiles file size
        metrics::record_pmtiles_file_size(file_content.len() as u64);

        // Compute SHA-256 hash of the file content (first 16 hex chars)
        let hash = {
            let mut hasher = Sha256::new();
            hasher.update(&file_content);
            let result = hasher.finalize();
            format!("{result:x}")[..16].to_string()
        };

        let new_s3_key = format!("activities/{}/{hash}.pmtiles", self.user_id);

        // Upload to activities S3 bucket with hash-based key
        match self
            .s3_client
            .put_object()
            .bucket(&self.activities_bucket)
            .key(&new_s3_key)
            .body(ByteStream::from(file_content))
            .content_type("application/vnd.pmtiles")
            .send()
            .await
        {
            Ok(_) => {
                metrics::increment_s3_upload_success();
                info!("Successfully uploaded PMTiles to activities S3: {new_s3_key}");
            }
            Err(e) => {
                metrics::increment_s3_upload_failure();
                return Err(anyhow::anyhow!("Failed to upload PMTiles to S3: {e}"));
            }
        }

        // Read current pmtilesKey from the users table
        let old_key = self.get_current_pmtiles_key().await?;

        // Update the users table with the new pmtilesKey
        self.update_pmtiles_key(&new_s3_key).await?;

        // Tag old S3 object for expiration if it differs from the new one
        if let Some(old_key) = old_key
            && old_key != new_s3_key
        {
            self.tag_for_expiration(&old_key).await;
        }

        Ok(())
    }

    async fn get_current_pmtiles_key(&self) -> Result<Option<String>> {
        let result = self
            .dynamodb_client
            .get_item()
            .table_name(&self.users_table_name)
            .key("id", AttributeValue::S(self.user_id.clone()))
            .projection_expression("pmtilesKey")
            .send()
            .await
            .context("Failed to read user record from DynamoDB")?;

        Ok(result
            .item
            .and_then(|item| item.get("pmtilesKey").cloned())
            .and_then(|v| match v {
                AttributeValue::S(s) => Some(s),
                _ => None,
            }))
    }

    async fn update_pmtiles_key(&self, new_key: &str) -> Result<()> {
        self.dynamodb_client
            .update_item()
            .table_name(&self.users_table_name)
            .key("id", AttributeValue::S(self.user_id.clone()))
            .update_expression("SET pmtilesKey = :key")
            .expression_attribute_values(":key", AttributeValue::S(new_key.to_string()))
            .send()
            .await
            .context("Failed to update pmtilesKey in DynamoDB")?;

        info!("Updated pmtilesKey for user {} to {new_key}", self.user_id);
        Ok(())
    }

    async fn tag_for_expiration(&self, s3_key: &str) {
        info!("Tagging old PMTiles object for expiration: {s3_key}");

        let tag = aws_sdk_s3::types::Tag::builder()
            .key("status")
            .value("expired")
            .build()
            .expect("valid tag");

        let tagging = aws_sdk_s3::types::Tagging::builder()
            .tag_set(tag)
            .build()
            .expect("valid tagging");

        if let Err(e) = self
            .s3_client
            .put_object_tagging()
            .bucket(&self.activities_bucket)
            .key(s3_key)
            .tagging(tagging)
            .send()
            .await
        {
            error!("Failed to tag old PMTiles object {s3_key} for expiration: {e}");
        }
    }
}
