use crate::intervals_client::Activity;
use crate::metrics_helper;
use anyhow::Result;
use aws_sdk_s3::Client as S3Client;
use aws_sdk_s3::primitives::ByteStream;
use function_timer::time;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::{Read, Write};
use tracing::{error, info};

#[derive(Debug, Serialize, Deserialize)]
pub struct ActivityArchiveEntry {
    pub activity_id: String,
    pub activity_hash: String,
    pub geojson_data: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ActivityArchive {
    pub athlete_id: String,
    pub last_updated: String,
    pub entries: HashMap<String, ActivityArchiveEntry>,
}

impl ActivityArchive {
    pub fn extract_geojson_data(&self) -> Vec<String> {
        let geojson_entries: Vec<String> = self
            .entries
            .values()
            .filter_map(|entry| entry.geojson_data.as_ref())
            .cloned()
            .collect();

        info!(
            "Extracted {} GeoJSON entries from archive",
            geojson_entries.len()
        );
        geojson_entries
    }
}

pub struct ActivityArchiveManager {
    athlete_id: String,
    archive: ActivityArchive,
}

impl ActivityArchiveManager {
    /// Create a new empty archive manager for building a fresh archive
    pub fn new_empty(athlete_id: String) -> Self {
        let archive = ActivityArchive {
            athlete_id: athlete_id.clone(),
            last_updated: chrono::Utc::now().to_rfc3339(),
            entries: HashMap::new(),
        };

        Self {
            athlete_id,
            archive,
        }
    }

    /// Load existing archive from S3 (returns the raw ActivityArchive)
    pub async fn load_existing(
        s3_client: &S3Client,
        s3_bucket: &str,
        athlete_id: &str,
    ) -> Result<ActivityArchive> {
        let archive_key = format!("athletes/{athlete_id}/activities.archive");

        Self::load_archive(s3_client, s3_bucket, &archive_key).await
    }

    #[time("load_archive_duration")]
    async fn load_archive(
        s3_client: &S3Client,
        s3_bucket: &str,
        archive_key: &str,
    ) -> Result<ActivityArchive> {
        match s3_client
            .get_object()
            .bucket(s3_bucket)
            .key(archive_key)
            .send()
            .await
        {
            Ok(response) => {
                metrics_helper::increment_s3_upload_success();
                let compressed_data = response.body.collect().await?.to_vec();

                // Decompress
                let mut decoder = zstd::Decoder::new(&compressed_data[..])?;
                let mut decompressed_data = Vec::new();
                decoder.read_to_end(&mut decompressed_data)?;

                // Deserialize
                let archive: ActivityArchive = bincode::deserialize(&decompressed_data)?;
                info!("Loaded archive with {} activities", archive.entries.len());
                Ok(archive)
            }
            Err(e) => {
                metrics_helper::increment_s3_upload_failure();
                Err(e.into())
            }
        }
    }

    #[time("save_archive_duration")]
    async fn save_archive(&self, s3_client: &S3Client, s3_bucket: &str) -> Result<()> {
        info!(
            "Saving archive with {} activities",
            self.archive.entries.len()
        );

        // Serialize
        let serialized_data = bincode::serialize(&self.archive)?;

        // Compress
        let mut encoder = zstd::Encoder::new(Vec::new(), 3)?; // Compression level 3
        encoder.write_all(&serialized_data)?;
        let compressed_data = encoder.finish()?;

        // Record compression metrics
        let compression_ratio = compressed_data.len() as f64 / serialized_data.len() as f64;
        metrics_helper::record_archive_compression_ratio(compression_ratio);
        metrics_helper::record_archive_size_bytes(compressed_data.len() as u64);

        info!(
            "Archive compressed from {} to {} bytes (ratio: {:.2})",
            serialized_data.len(),
            compressed_data.len(),
            compression_ratio
        );

        // Upload to S3
        let archive_key = format!("athletes/{}/activities.archive", self.athlete_id);
        match s3_client
            .put_object()
            .bucket(s3_bucket)
            .key(&archive_key)
            .body(ByteStream::from(compressed_data))
            .content_type("application/octet-stream")
            .send()
            .await
        {
            Ok(_) => {
                metrics_helper::increment_s3_upload_success();
                info!("Archive saved to S3: {}", archive_key);
                Ok(())
            }
            Err(e) => {
                metrics_helper::increment_s3_upload_failure();
                error!("Failed to save archive to S3: {}", e);
                Err(e.into())
            }
        }
    }

    /// Transfer an unchanged entry from existing archive to new archive (zero-copy)
    pub fn transfer_unchanged_entry(
        &mut self,
        existing_archive: &mut ActivityArchive,
        activity: &Activity,
    ) -> bool {
        let activity_hash = Self::compute_activity_hash(activity);

        // Check if activity exists with same hash in existing archive
        if let Some(existing_entry) = existing_archive.entries.get(&activity.id)
            && existing_entry.activity_hash == activity_hash
        {
            // Zero-copy transfer: remove from old archive and insert into new
            if let Some(entry) = existing_archive.entries.remove(&activity.id) {
                self.archive.entries.insert(activity.id.clone(), entry);
                return true;
            }
        }
        false
    }

    /// Add a new or changed activity to the archive
    pub fn add_new_activity(&mut self, geojson: Option<String>, activity: &Activity) -> Result<()> {
        let activity_hash = Self::compute_activity_hash(activity);

        let entry = ActivityArchiveEntry {
            activity_id: activity.id.clone(),
            activity_hash,
            geojson_data: geojson,
        };

        self.archive.entries.insert(activity.id.clone(), entry);

        Ok(())
    }

    pub async fn finalize(&mut self, s3_client: &S3Client, s3_bucket: &str) -> Result<()> {
        // Update timestamp only when finalizing
        self.archive.last_updated = chrono::Utc::now().to_rfc3339();
        self.save_archive(s3_client, s3_bucket).await
    }

    #[time("extract_geojson_data_duration")]
    pub fn extract_geojson_data(&self) -> Vec<String> {
        let geojson_entries: Vec<String> = self
            .archive
            .entries
            .values()
            .filter_map(|entry| entry.geojson_data.as_ref())
            .cloned()
            .collect();

        info!(
            "Extracted {} GeoJSON entries from archive",
            geojson_entries.len()
        );
        geojson_entries
    }

    pub fn get_archive_stats(&self) -> (usize, usize) {
        let total_entries = self.archive.entries.len();
        let geojson_entries = self
            .archive
            .entries
            .values()
            .filter(|entry| entry.geojson_data.is_some())
            .count();

        (total_entries, geojson_entries)
    }

    /// Extend this archive with entries from another archive
    pub fn extend(&mut self, other: ActivityArchiveManager) {
        self.archive.entries.extend(other.archive.entries);
    }

    pub fn compute_activity_hash(activity: &Activity) -> String {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let mut hasher = DefaultHasher::new();
        activity.id.hash(&mut hasher);
        activity.name.hash(&mut hasher);
        activity.start_date_local.hash(&mut hasher);
        activity.elapsed_time.hash(&mut hasher);
        if let Some(distance) = activity.distance {
            distance.to_bits().hash(&mut hasher);
        }

        format!("{:x}", hasher.finish())
    }
}
