use super::ActivitySync;
use crate::intervals_client::Activity;
use crate::metrics_helper;
use anyhow::Result;
use aws_sdk_s3::primitives::ByteStream;
use function_timer::time;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Write};
use tracing::{error, info};

#[derive(Debug, Serialize, Deserialize)]
pub struct ActivityIndex {
    pub athlete_id: String,
    pub last_updated: String,
    pub geojson_activities: HashSet<String>,
    pub empty_activities: HashSet<String>,
}

impl ActivityIndex {
    pub fn insert_geojson(&mut self, activity_id: &str, activity_hash: &str) {
        let key = Self::create_key(activity_id, activity_hash);
        self.geojson_activities.insert(key);
    }

    pub fn insert_empty(&mut self, activity_id: &str, activity_hash: &str) {
        let key = Self::create_key(activity_id, activity_hash);
        self.empty_activities.insert(key);
    }

    pub fn total_activities(&self) -> usize {
        self.geojson_activities.len() + self.empty_activities.len()
    }

    pub fn new_empty(athlete_id: String) -> Self {
        Self {
            athlete_id,
            last_updated: chrono::Utc::now().to_rfc3339(),
            geojson_activities: HashSet::new(),
            empty_activities: HashSet::new(),
        }
    }

    pub fn try_copy(&self, activity: &Activity, target: &mut ActivityIndex) -> bool {
        let activity_hash = activity.compute_hash();
        let key = Self::create_key(&activity.id, &activity_hash);
        
        if self.geojson_activities.contains(&key) {
            target.geojson_activities.insert(key);
            true
        } else if self.empty_activities.contains(&key) {
            target.empty_activities.insert(key);
            true
        } else {
            false
        }
    }

    pub fn create_key(activity_id: &str, activity_hash: &str) -> String {
        format!("{activity_id}:{activity_hash}")
    }
}

impl ActivitySync {
    /// Load existing activity index from S3 (returns the raw ActivityIndex)
    #[time("download_index_duration")]
    pub async fn download_index(&self) -> Result<ActivityIndex> {
        let index_key = format!("athletes/{}/activities.index", self.athlete_id);

        match self
            .s3_client
            .get_object()
            .bucket(&self.s3_bucket)
            .key(index_key)
            .send()
            .await
        {
            Ok(response) => {
                metrics_helper::increment_s3_upload_success();
                let index_data = response.body.collect().await?.to_vec();

                // Deserialize
                let index: ActivityIndex = bincode::deserialize(&index_data)?;
                info!("Loaded index with {} total activities ({} geojson, {} empty)", 
                    index.total_activities(),
                    index.geojson_activities.len(),
                    index.empty_activities.len());
                Ok(index)
            }
            Err(e) => {
                metrics_helper::increment_s3_upload_failure();
                Err(e.into())
            }
        }
    }

    #[time("upload_index_duration")]
    async fn upload_index(&self, index: &ActivityIndex) -> Result<()> {
        info!("Saving index with {} total activities ({} geojson, {} empty)", 
            index.total_activities(),
            index.geojson_activities.len(), 
            index.empty_activities.len());

        // Serialize
        let serialized_data = bincode::serialize(index)?;
        
        // Record index size metrics
        metrics_helper::record_index_size_bytes(serialized_data.len() as u64);

        // Upload to S3
        let index_key = format!("athletes/{}/activities.index", index.athlete_id);
        match self
            .s3_client
            .put_object()
            .bucket(&self.s3_bucket)
            .key(&index_key)
            .body(ByteStream::from(serialized_data))
            .content_type("application/octet-stream")
            .send()
            .await
        {
            Ok(_) => {
                metrics_helper::increment_s3_upload_success();
                info!("Index saved to S3: {}", index_key);
                Ok(())
            }
            Err(e) => {
                metrics_helper::increment_s3_upload_failure();
                error!("Failed to save index to S3: {}", e);
                Err(e.into())
            }
        }
    }


    /// Parse activity filename to extract ID, hash, and extension
    /// Expected format: activity_{id}_{hash}.{extension}
    fn parse_activity_filename(file_path: &std::path::Path) -> Option<(String, String, String)> {
        if let Some(file_name) = file_path.file_stem().and_then(|s| s.to_str()) {
            if let Some(info_part) = file_name.strip_prefix("activity_") {
                if let Some(last_underscore) = info_part.rfind('_') {
                    let activity_id = &info_part[..last_underscore];
                    let activity_hash = &info_part[last_underscore + 1..];
                    if let Some(extension) = file_path.extension().and_then(|s| s.to_str()) {
                        return Some((
                            activity_id.to_string(),
                            activity_hash.to_string(),
                            extension.to_string(),
                        ));
                    }
                }
            }
        }
        None
    }

    /// Finalize archive by streaming existing activities and appending new ones from temp directory
    /// Returns the path to the uncompressed concatenated GeoJSON file
    #[time("finalize_archive_duration")]
    pub async fn finalize_archive(
        &self,
        temp_dir_path: &std::path::Path,
        mut copied_index: ActivityIndex,
    ) -> Result<std::path::PathBuf> {
        // Update timestamp on copied index
        copied_index.last_updated = chrono::Utc::now().to_rfc3339();

        // Create temporary file for new GeoJSON content in work directory
        let temp_geojson_path = self.work_dir.join(format!("activities_{}.geojson", self.athlete_id));
        let temp_geojson_file = File::create(&temp_geojson_path)?;
        let mut geojson_writer = std::io::BufWriter::new(temp_geojson_file);

        let mut new_activities = 0;
        let mut copied_activities = 0;

        info!("Creating new activity index. Beginning with {} ({} geojson, {} empty) activities from existing index.",
            copied_index.total_activities(), copied_index.geojson_activities.len(), copied_index.empty_activities.len());

        // Copy existing GeoJSON activities from the existing archive
        if !copied_index.geojson_activities.is_empty() {
            if let Ok(existing_geojson_content) = self.download_existing_geojson().await {
                let reader = BufReader::new(existing_geojson_content.as_bytes());

                for line_result in reader.lines() {
                    let line = line_result?;
                    match serde_json::from_str::<serde_json::Value>(&line) {
                        Ok(feature) => {
                            if let Some(properties) = feature.get("properties") {
                                if let Some(activity_id) = properties.get("id").and_then(|v| v.as_str()) {
                                    if let Some(activity_hash) = properties.get("activity_hash").and_then(|v| v.as_str()) {
                                        let key = ActivityIndex::create_key(activity_id, activity_hash);
                                        if copied_index.geojson_activities.contains(&key) {
                                            // Copy this activity to new file
                                            writeln!(geojson_writer, "{line}")?;
                                            copied_activities += 1;
                                        }
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            error!("Failed to parse JSON from existing activity line: {}", e);
                            error!("Problematic line: {}", line);
                        }
                    }
                }
            }
        }

        info!("Copied geojson data for {} activities", copied_activities);

        // Add new activities from temp directory
        if let Ok(entries) = std::fs::read_dir(temp_dir_path) {
            for entry in entries.flatten() {
                let file_path = entry.path();

                // Parse activity ID, hash, and extension from filename
                if let Some((activity_id, activity_hash, extension)) =
                    Self::parse_activity_filename(&file_path)
                {
                    match extension.as_str() {
                        "geojson" => {
                            if let Ok(geojson_content) = std::fs::read_to_string(&file_path) {
                                writeln!(geojson_writer, "{}", geojson_content.trim())?;
                                new_activities += 1;
                            }
                            copied_index.insert_geojson(&activity_id, &activity_hash);
                        }
                        "stub" => {
                            // Empty activity - add to empty_activities set
                            copied_index.insert_empty(&activity_id, &activity_hash);
                        }
                        _ => {
                            error!("Unknown extension {}", extension);
                            continue;
                        }
                    }
                }

                // Clean up temp file immediately after processing
                std::fs::remove_file(&file_path).ok();
            }
        }

        info!(
            "Finalizing archive with {} total activities ({} geojson, {} empty), {} new activities processed",
            copied_index.total_activities(),
            copied_index.geojson_activities.len(),
            copied_index.empty_activities.len(),
            new_activities,
        );

        // Flush and close the writer
        drop(geojson_writer);

        // Compress and upload GeoJSON file
        self.upload_compressed_geojson(&temp_geojson_path).await?;

        // Save index
        self.upload_index(&copied_index).await?;

        // Clean up temp directory (but keep the final GeoJSON file)
        std::fs::remove_dir_all(temp_dir_path).ok();

        // Return path to uncompressed GeoJSON file for tile generation
        Ok(temp_geojson_path)
    }

    /// Load existing GeoJSON content from S3 (decompressed)
    #[time("download_existing_geojson_duration")]
    async fn download_existing_geojson(&self) -> Result<String> {
        let geojson_key = format!("athletes/{}/activities.geojson.zst", self.athlete_id);

        let response = self
            .s3_client
            .get_object()
            .bucket(&self.s3_bucket)
            .key(&geojson_key)
            .send()
            .await?;

        let compressed_data = response.body.collect().await?.to_vec();

        // Decompress
        let mut decoder = zstd::Decoder::new(&compressed_data[..])?;
        let mut decompressed_data = String::new();
        decoder.read_to_string(&mut decompressed_data)?;

        Ok(decompressed_data)
    }

    /// Upload compressed GeoJSON file to S3
    #[time("upload_compressed_geojson_duration")]
    async fn upload_compressed_geojson(&self, temp_file_path: &str) -> Result<()> {
        // Read temp file and compress
        let file_content = std::fs::read(temp_file_path)?;

        let mut encoder = zstd::Encoder::new(Vec::new(), 3)?; // Compression level 3
        encoder.write_all(&file_content)?;
        let compressed_data = encoder.finish()?;

        // Record compression metrics
        let compression_ratio = compressed_data.len() as f64 / file_content.len() as f64;
        metrics_helper::record_archive_compression_ratio(compression_ratio);
        metrics_helper::record_archive_size_bytes(compressed_data.len() as u64);

        info!(
            "GeoJSON compressed from {} to {} bytes (ratio: {:.2})",
            file_content.len(),
            compressed_data.len(),
            compression_ratio
        );

        // Upload to S3
        let geojson_key = format!("athletes/{}/activities.geojson.zst", self.athlete_id);
        self.s3_client
            .put_object()
            .bucket(&self.s3_bucket)
            .key(&geojson_key)
            .body(ByteStream::from(compressed_data))
            .content_type("application/octet-stream")
            .send()
            .await?;

        info!("Compressed GeoJSON saved to S3: {}", geojson_key);
        Ok(())
    }
}
