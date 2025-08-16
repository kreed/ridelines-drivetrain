use crate::intervals_client::Activity;
use crate::metrics_helper;
use anyhow::Result;
use aws_sdk_s3::Client as S3Client;
use aws_sdk_s3::primitives::ByteStream;
use function_timer::time;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::fs::File;
use tracing::{error, info};

#[derive(Debug, Serialize, Deserialize)]
pub struct ActivityIndexEntry {
    pub activity_id: String,
    pub activity_hash: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ActivityIndex {
    pub athlete_id: String,
    pub last_updated: String,
    pub entries: HashMap<String, ActivityIndexEntry>,
}


impl ActivityIndex {
    pub fn contains_activity_with_hash(&self, activity_id: &str, activity_hash: &str) -> bool {
        self.entries
            .get(activity_id)
            .map(|entry| entry.activity_hash == activity_hash)
            .unwrap_or(false)
    }

}

pub struct ActivityArchiveManager {
    athlete_id: String,
}

impl ActivityArchiveManager {
    /// Create a new empty archive manager
    pub fn new_empty(athlete_id: String) -> Self {
        Self {
            athlete_id,
        }
    }

    /// Load existing activity index from S3 (returns the raw ActivityIndex)
    pub async fn load_existing_index(
        s3_client: &S3Client,
        s3_bucket: &str,
        athlete_id: &str,
    ) -> Result<ActivityIndex> {
        let index_key = format!("athletes/{athlete_id}/activities.index");

        Self::load_index(s3_client, s3_bucket, &index_key).await
    }

    #[time("load_index_duration")]
    async fn load_index(
        s3_client: &S3Client,
        s3_bucket: &str,
        index_key: &str,
    ) -> Result<ActivityIndex> {
        match s3_client
            .get_object()
            .bucket(s3_bucket)
            .key(index_key)
            .send()
            .await
        {
            Ok(response) => {
                metrics_helper::increment_s3_upload_success();
                let index_data = response.body.collect().await?.to_vec();

                // Deserialize
                let index: ActivityIndex = bincode::deserialize(&index_data)?;
                info!("Loaded index with {} activities", index.entries.len());
                Ok(index)
            }
            Err(e) => {
                metrics_helper::increment_s3_upload_failure();
                Err(e.into())
            }
        }
    }

    #[time("save_index_duration")]
    async fn save_index(index: &ActivityIndex, s3_client: &S3Client, s3_bucket: &str) -> Result<()> {
        info!(
            "Saving index with {} activities",
            index.entries.len()
        );

        // Serialize
        let serialized_data = bincode::serialize(index)?;

        // Upload to S3
        let index_key = format!("athletes/{}/activities.index", index.athlete_id);
        match s3_client
            .put_object()
            .bucket(s3_bucket)
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

    /// Check if activity exists with same hash in index (activity is unchanged)
    pub fn is_activity_unchanged(
        index: &ActivityIndex,
        activity: &Activity,
    ) -> bool {
        let activity_hash = activity.compute_hash();
        index.contains_activity_with_hash(&activity.id, &activity_hash)
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
                        return Some((activity_id.to_string(), activity_hash.to_string(), extension.to_string()));
                    }
                }
            }
        }
        None
    }

    /// Finalize archive by streaming existing activities and appending new ones from temp directory
    /// Returns the path to the uncompressed concatenated GeoJSON file
    pub async fn finalize_archive(
        &self,
        existing_activity_ids: &[String],
        temp_dir_path: &str,
        s3_client: &S3Client,
        s3_bucket: &str,
    ) -> Result<String> {
        // Count new activities in temp directory
        let new_activity_count = std::fs::read_dir(temp_dir_path)
            .map(|entries| entries.count())
            .unwrap_or(0);
            
        info!(
            "Finalizing archive with {} existing activities and {} new activities from temp dir",
            existing_activity_ids.len(),
            new_activity_count
        );

        // Create new index with all activities
        let mut new_index = ActivityIndex {
            athlete_id: self.athlete_id.clone(),
            last_updated: chrono::Utc::now().to_rfc3339(),
            entries: HashMap::new(),
        };

        // Create temporary file for new GeoJSON content
        let temp_geojson_path = format!("/tmp/activities_{}.geojson", self.athlete_id);
        let temp_geojson_file = File::create(&temp_geojson_path)?;
        let mut geojson_writer = std::io::BufWriter::new(temp_geojson_file);

        // Copy existing activities that are still present
        if !existing_activity_ids.is_empty() {
            if let Ok(existing_geojson_content) = self.load_existing_geojson(s3_client, s3_bucket).await {
                let reader = BufReader::new(existing_geojson_content.as_bytes());
                
                for line in reader.lines() {
                    let line = line?;
                    if let Ok(feature) = serde_json::from_str::<serde_json::Value>(&line) {
                        if let Some(properties) = feature.get("properties") {
                            if let Some(activity_id) = properties.get("id").and_then(|v| v.as_str()) {
                                if existing_activity_ids.contains(&activity_id.to_string()) {
                                    // Copy this activity to new file
                                    writeln!(geojson_writer, "{line}")?;
                                    
                                    // Add to index
                                    if let Some(activity_hash) = properties.get("activity_hash").and_then(|v| v.as_str()) {
                                        let index_entry = ActivityIndexEntry {
                                            activity_id: activity_id.to_string(),
                                            activity_hash: activity_hash.to_string(),
                                        };
                                        new_index.entries.insert(activity_id.to_string(), index_entry);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        // Add new activities from temp directory
        if let Ok(entries) = std::fs::read_dir(temp_dir_path) {
            for entry in entries.flatten() {
                let file_path = entry.path();
                
                // Parse activity ID, hash, and extension from filename
                if let Some((activity_id, activity_hash, extension)) = Self::parse_activity_filename(&file_path) {
                    match extension.as_str() {
                        "geojson" => {
                            // Read and stream GeoJSON file
                            if let Ok(geojson_content) = std::fs::read_to_string(&file_path) {
                                writeln!(geojson_writer, "{}", geojson_content.trim())?;
                                info!("Added GeoJSON activity {} to archive", activity_id);
                            }
                        }
                        "stub" => {
                            // Stub file is empty, just note it was processed
                            info!("Added stub activity {} to archive", activity_id);
                        }
                        _ => {
                            // Unknown extension, skip
                            continue;
                        }
                    }
                    
                    // Add to index using filename info
                    let index_entry = ActivityIndexEntry {
                        activity_id: activity_id.clone(),
                        activity_hash,
                    };
                    new_index.entries.insert(activity_id, index_entry);
                }
                
                // Clean up temp file immediately after processing
                std::fs::remove_file(&file_path).ok();
            }
        }

        // Flush and close the writer
        drop(geojson_writer);

        // Compress and upload GeoJSON file
        self.upload_compressed_geojson(&temp_geojson_path, s3_client, s3_bucket).await?;

        // Save index
        Self::save_index(&new_index, s3_client, s3_bucket).await?;

        // Clean up temp directory (but keep the final GeoJSON file)
        std::fs::remove_dir_all(temp_dir_path).ok();

        // Return path to uncompressed GeoJSON file for tile generation
        Ok(temp_geojson_path)
    }

    /// Load existing GeoJSON content from S3 (decompressed)
    async fn load_existing_geojson(&self, s3_client: &S3Client, s3_bucket: &str) -> Result<String> {
        let geojson_key = format!("athletes/{}/activities.geojson.zst", self.athlete_id);
        
        let response = s3_client
            .get_object()
            .bucket(s3_bucket)
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
    async fn upload_compressed_geojson(&self, temp_file_path: &str, s3_client: &S3Client, s3_bucket: &str) -> Result<()> {
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
        s3_client
            .put_object()
            .bucket(s3_bucket)
            .key(&geojson_key)
            .body(ByteStream::from(compressed_data))
            .content_type("application/octet-stream")
            .send()
            .await?;

        info!("Compressed GeoJSON saved to S3: {}", geojson_key);
        Ok(())
    }



}
