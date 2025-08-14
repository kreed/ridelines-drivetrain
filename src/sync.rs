use crate::convert::convert_fit_to_geojson;
use crate::intervals_client::{Activity, DownloadError, IntervalsClient};
use aws_sdk_s3::Client as S3Client;
use aws_sdk_s3::primitives::ByteStream;
use futures::stream::{self, StreamExt};
use sanitize_filename::sanitize;
use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use tokio::sync::Semaphore;
use tracing::{info, warn, error};

#[derive(Debug)]
pub struct DownloadStats {
    pub downloaded: usize,
    pub skipped_unchanged: usize,
    pub downloaded_empty: usize,
    pub failed: usize,
    pub deleted: usize,
}

pub struct SyncJob {
    intervals_client: IntervalsClient,
    s3_client: S3Client,
    s3_bucket: String,
    athlete_id: String,
    s3_prefix: String,
    stats: Arc<Mutex<DownloadStats>>,
    semaphore: Arc<Semaphore>,
}

impl SyncJob {
    pub fn new(api_key: &str, athlete_id: &str, s3_client: S3Client, s3_bucket: &str) -> Self {
        let s3_prefix = format!("athletes/{athlete_id}");
        
        Self {
            intervals_client: IntervalsClient::new(api_key.to_string()),
            s3_client,
            s3_bucket: s3_bucket.to_string(),
            athlete_id: athlete_id.to_string(),
            s3_prefix,
            stats: Arc::new(Mutex::new(DownloadStats {
                downloaded: 0,
                skipped_unchanged: 0,
                downloaded_empty: 0,
                failed: 0,
                deleted: 0,
            })),
            semaphore: Arc::new(Semaphore::new(5)),
        }
    }

    pub async fn sync_activities(&self) -> Result<(), Box<dyn std::error::Error>> {
        info!("Starting sync to S3 bucket: {}, athlete: {}", self.s3_bucket, self.athlete_id);
        
        let activities = self.fetch_activities().await?;
        if activities.is_empty() {
            info!("No activities found for athlete {}", self.athlete_id);
            return Ok(());
        }
        
        info!("Found {} activities for athlete {}", activities.len(), self.athlete_id);
        
        let orphaned_files = self.get_existing_files().await?;
        self.process_activities_batch(activities, orphaned_files.clone()).await;
        self.cleanup_orphaned_files(orphaned_files).await;
        self.concatenate_geojson_files().await;
        self.report_results();
        
        Ok(())
    }

    async fn fetch_activities(&self) -> Result<Vec<Activity>, Box<dyn std::error::Error>> {
        match self.intervals_client.fetch_activities(&self.athlete_id).await {
            Ok(activities) => Ok(activities),
            Err(e) => {
                error!("Error fetching activities: {e}");
                Err(e)
            }
        }
    }

    async fn get_existing_files(&self) -> Result<Arc<Mutex<HashSet<String>>>, Box<dyn std::error::Error>> {
        let files = self.list_activity_files(&[".geojson", ".stub"]).await
            .unwrap_or_else(|e| {
                warn!("Failed to get existing S3 files: {e}");
                Vec::new()
            })
            .into_iter()
            .collect();
        
        Ok(Arc::new(Mutex::new(files)))
    }

    async fn list_activity_files(&self, extensions: &[&str]) -> Result<Vec<String>, Box<dyn std::error::Error>> {
        let files = self.s3_client
            .list_objects_v2()
            .bucket(&self.s3_bucket)
            .prefix(&self.s3_prefix)
            .into_paginator()
            .send()
            .try_collect()
            .await?
            .into_iter()
            .flat_map(|output| output.contents.unwrap_or_default())
            .filter_map(|object| object.key)
            .filter(|key| extensions.iter().any(|ext| key.ends_with(ext)))
            .collect();
        
        Ok(files)
    }

    async fn process_activities_batch(&self, activities: Vec<Activity>, orphaned_files: Arc<Mutex<HashSet<String>>>) {
        info!("Starting sync of {} activities", activities.len());
        
        stream::iter(activities)
            .map(|activity| {
                let orphaned_files = orphaned_files.clone();
                self.process_single_activity(activity, orphaned_files)
            })
            .buffer_unordered(5)
            .collect::<Vec<_>>()
            .await;
            
        info!("Activity processing complete");
    }

    async fn process_single_activity(&self, activity: Activity, orphaned_files: Arc<Mutex<HashSet<String>>>) {
        let _permit = self.semaphore.acquire().await.unwrap();
        
        info!("Processing activity: {} (ID: {})", activity.name, activity.id);
        
        let geojson_key = self.generate_key(&activity, "geojson");
        let stub_key = self.generate_key(&activity, "stub");
        
        let geojson_exists = orphaned_files.lock().unwrap().contains(&geojson_key);
        let stub_exists = orphaned_files.lock().unwrap().contains(&stub_key);
        
        // Check for bad state (both files exist) - redownload
        if geojson_exists && stub_exists {
            warn!("Both .geojson and .stub files exist for activity {}, redownloading", activity.id);
        } else if geojson_exists || stub_exists {
            // One file exists, skip and remove both types from orphaned files
            if let Ok(mut stats) = self.stats.lock() {
                stats.skipped_unchanged += 1;
            }
            if let Ok(mut orphaned) = orphaned_files.lock() {
                orphaned.remove(&geojson_key);
                orphaned.remove(&stub_key);
            }
            return;
        }
        
        // Neither file exists, download activity
        self.download_and_process_activity(&activity, &geojson_key, &stub_key, orphaned_files).await;
    }

    async fn download_and_process_activity(
        &self, 
        activity: &Activity, 
        geojson_key: &str, 
        stub_key: &str, 
        orphaned_files: Arc<Mutex<HashSet<String>>>
    ) {
        // Helper function to write empty stub file to S3 and update stats
        let write_empty_file = || async {
            let result = self.s3_client
                .put_object()
                .bucket(&self.s3_bucket)
                .key(stub_key)
                .body(ByteStream::from_static(b""))
                .send()
                .await;
            
            match result {
                Ok(_) => {
                    if let Ok(mut stats) = self.stats.lock() {
                        stats.downloaded_empty += 1;
                    }
                    // Remove the stub file from orphaned files list
                    if let Ok(mut orphaned) = orphaned_files.lock() {
                        orphaned.remove(stub_key);
                    }
                }
                Err(e) => {
                    error!("Failed to write stub file to S3 for activity {}: {}", activity.id, e);
                    if let Ok(mut stats) = self.stats.lock() {
                        stats.failed += 1;
                    }
                }
            }
        };
        
        // Download with retry logic handled by middleware
        match self.intervals_client.download_fit(&activity.id).await {
            Ok(fit_data) => {
                // Convert FIT to GeoJSON
                match convert_fit_to_geojson(&fit_data, activity).await {
                    Ok(Some(data)) => {
                        self.write_geojson_to_s3(geojson_key, data, orphaned_files).await;
                    }
                    Ok(None) => {
                        // No GPS data found in FIT file - write empty file
                        write_empty_file().await;
                    }
                    Err(e) => {
                        error!("Failed to convert FIT to GeoJSON for activity {}: {}", activity.id, e);
                        if let Ok(mut stats) = self.stats.lock() {
                            stats.failed += 1;
                        }
                    }
                }
            }
            Err(DownloadError::Http(status)) if status.as_u16() == 422 => {
                // HTTP 422 usually means no GPS data available - write empty file
                write_empty_file().await;
            }
            Err(e) => {
                error!("Failed to download activity {}: {}", activity.id, e);
                if let Ok(mut stats) = self.stats.lock() {
                    stats.failed += 1;
                }
            }
        }
    }

    async fn write_geojson_to_s3(&self, geojson_key: &str, data: String, orphaned_files: Arc<Mutex<HashSet<String>>>) {
        let result = self.s3_client
            .put_object()
            .bucket(&self.s3_bucket)
            .key(geojson_key)
            .body(ByteStream::from(data.into_bytes()))
            .content_type("application/geo+json")
            .send()
            .await;
        
        match result {
            Ok(_) => {
                if let Ok(mut stats) = self.stats.lock() {
                    stats.downloaded += 1;
                }
                // Remove the geojson file from orphaned files list
                if let Ok(mut orphaned) = orphaned_files.lock() {
                    orphaned.remove(geojson_key);
                }
            }
            Err(e) => {
                error!("Failed to write GeoJSON file to S3 for activity: {}", e);
                if let Ok(mut stats) = self.stats.lock() {
                    stats.failed += 1;
                }
            }
        }
    }

    async fn cleanup_orphaned_files(&self, orphaned_files: Arc<Mutex<HashSet<String>>>) {
        // Delete all remaining orphaned files from S3
        let orphaned_files_to_delete: Vec<String> = {
            let guard = orphaned_files.lock().unwrap();
            guard.iter().cloned().collect()
        };
        let deleted_count = orphaned_files_to_delete.len();
        for s3_key in &orphaned_files_to_delete {
            let result = self.s3_client
                .delete_object()
                .bucket(&self.s3_bucket)
                .key(s3_key)
                .send()
                .await;
                
            if let Err(e) = result {
                warn!("Failed to delete orphaned S3 object {s3_key}: {e}");
            } else {
                info!("Deleted orphaned S3 object: {s3_key}");
            }
        }
        
        // Update deleted count in stats
        if let Ok(mut stats) = self.stats.lock() {
            stats.deleted = deleted_count;
        }
    }

    async fn concatenate_geojson_files(&self) {
        info!("Starting GeoJSON file concatenation");
        
        // Get all .geojson files
        let geojson_files = match self.list_activity_files(&[".geojson"]).await {
            Ok(files) => files,
            Err(e) => {
                error!("Failed to list GeoJSON files for concatenation: {}", e);
                return;
            }
        };
        
        if geojson_files.is_empty() {
            info!("No GeoJSON files found to concatenate");
            return;
        }
        
        info!("Found {} GeoJSON files to concatenate", geojson_files.len());
        
        // Concatenate all file contents
        let mut concatenated_content = String::new();
        let mut successful_files = 0;
        
        for file_key in &geojson_files {
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
            warn!("No files could be read for concatenation");
            return;
        }
        
        // Write concatenated file to S3
        let concatenated_key = format!("athletes/{}/all-activities.dat", self.athlete_id);
        match self.write_concatenated_file(&concatenated_key, concatenated_content).await {
            Ok(_) => {
                info!("Successfully created concatenated file with {} activities", successful_files);
            }
            Err(e) => {
                error!("Failed to write concatenated file: {}", e);
            }
        }
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

    async fn write_concatenated_file(&self, key: &str, content: String) -> Result<(), Box<dyn std::error::Error>> {
        self.s3_client
            .put_object()
            .bucket(&self.s3_bucket)
            .key(key)
            .body(ByteStream::from(content.into_bytes()))
            .content_type("text/plain")
            .send()
            .await?;
            
        Ok(())
    }

    fn report_results(&self) {
        // Extract final stats
        let final_stats = self.stats.lock().unwrap();
        info!("Sync summary:");
        info!("  Downloaded: {}", final_stats.downloaded);
        info!("  Skipped (unchanged): {}", final_stats.skipped_unchanged);
        info!("  Downloaded (empty/no GPS): {}", final_stats.downloaded_empty);
        info!("  Deleted (obsolete): {}", final_stats.deleted);
        info!("  Errors: {}", final_stats.failed);
    }

    fn generate_key(&self, activity: &Activity, extension: &str) -> String {
        let date = parse_iso_date(&activity.start_date_local);
        let sanitized_name = sanitize(&activity.name);
        let sanitized_type = sanitize(&activity.activity_type);
        let distance_str = format_distance(activity.distance);
        let elapsed_time_str = format_elapsed_time(activity.elapsed_time);

        format!(
            "athletes/{}/{date}-{sanitized_name}-{sanitized_type}-{distance_str}-{elapsed_time_str}-{}.{extension}",
            self.athlete_id, activity.id
        )
    }
}

fn parse_iso_date(date_str: &str) -> String {
    // Extract just the date part (YYYY-MM-DD) from ISO datetime
    date_str.split('T').next().unwrap_or(date_str).to_string()
}

fn format_distance(distance: Option<f64>) -> String {
    match distance {
        Some(d) => format!("{:.2}km", d / 1000.0),
        None => "0.00km".to_string(),
    }
}

fn format_elapsed_time(elapsed_seconds: i64) -> String {
    let hours = elapsed_seconds / 3600;
    let minutes = (elapsed_seconds % 3600) / 60;
    let seconds = elapsed_seconds % 60;

    if hours > 0 {
        format!("{hours}h{minutes}m{seconds}s")
    } else if minutes > 0 {
        format!("{minutes}m{seconds}s")
    } else {
        format!("{seconds}s")
    }
}
