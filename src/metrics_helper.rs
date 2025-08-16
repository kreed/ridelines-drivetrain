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

pub fn increment_lambda_success() {
    counter!("lambda_total", "result" => "success").increment(1);
}

pub fn increment_lambda_failure() {
    counter!("lambda_total", "result" => "failure").increment(1);
}

/// Business Logic Metrics
pub fn increment_activities_with_gps(count: u64) {
    counter!("activities_with_gps").increment(count);
}

pub fn increment_activities_without_gps(count: u64) {
    counter!("activities_without_gps").increment(count);
}

pub fn increment_activities_skipped_unchanged(count: u64) {
    counter!("activities_skipped_unchanged").increment(count);
}

pub fn increment_activities_downloaded_new(count: u64) {
    counter!("activities_downloaded_new").increment(count);
}

pub fn increment_activities_failed(count: u64) {
    counter!("activities_failed").increment(count);
}

/// Resource Usage Metrics
pub fn record_pmtiles_file_size(size_bytes: u64) {
    gauge!("pmtiles_file_size_bytes").set(size_bytes as f64);
}


/// Archive-specific Metrics
pub fn record_archive_compression_ratio(ratio: f64) {
    gauge!("archive_compression_ratio").set(ratio);
}

pub fn record_archive_size_bytes(size_bytes: u64) {
    gauge!("archive_size_bytes").set(size_bytes as f64);
}
