use metrics::{counter, gauge};

/// Error/Reliability Metrics - Success/Failure pairs
pub fn increment_intervals_api_success() {
    counter!("intervals_api_total", "result" => "success").increment(1);
}

pub fn increment_intervals_api_failure() {
    counter!("intervals_api_total", "result" => "failure").increment(1);
}

pub fn increment_s3_upload_success() {
    counter!("s3_upload_total", "result" => "success").increment(1);
}

pub fn increment_s3_upload_failure() {
    counter!("s3_upload_total", "result" => "failure").increment(1);
}

pub fn increment_tippecanoe_success() {
    counter!("tippecanoe_total", "result" => "success").increment(1);
}

pub fn increment_tippecanoe_failure() {
    counter!("tippecanoe_total", "result" => "failure").increment(1);
}

pub fn increment_sqlite_success() {
    counter!("sqlite_total", "result" => "success").increment(1);
}

pub fn increment_sqlite_error() {
    counter!("sqlite_total", "result" => "failure").increment(1);
}

pub fn increment_lambda_success() {
    counter!("lambda_total", "result" => "success").increment(1);
}

pub fn increment_lambda_failure() {
    counter!("lambda_total", "result" => "failure").increment(1);
}

/// Business Logic Metrics
pub fn set_activities_with_gps_count(count: u64) {
    gauge!("activities_with_gps_count").set(count as f64);
}

pub fn set_activities_without_gps_count(count: u64) {
    gauge!("activities_without_gps_count").set(count as f64);
}

pub fn set_activities_skipped_unchanged(count: u64) {
    counter!("activities_skipped_unchanged").increment(count);
}

pub fn set_activities_downloaded_new(count: u64) {
    counter!("activities_downloaded_new").increment(count);
}


/// Resource Usage Metrics
pub fn record_mbtiles_file_size(size_bytes: u64) {
    gauge!("mbtiles_file_size_bytes").set(size_bytes as f64);
}

pub fn record_geojson_concatenated_size(size_bytes: u64) {
    gauge!("geojson_concatenated_size_bytes").set(size_bytes as f64);
}

pub fn set_total_tiles_generated(count: u64) {
    gauge!("total_tiles_generated").set(count as f64);
}

/// Archive-specific Metrics
pub fn record_archive_compression_ratio(ratio: f64) {
    gauge!("archive_compression_ratio").set(ratio);
}

pub fn record_archive_size_bytes(size_bytes: u64) {
    gauge!("archive_size_bytes").set(size_bytes as f64);
}
