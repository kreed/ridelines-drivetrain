use crate::intervals_client::IntervalsClient;
use aws_sdk_s3::Client as S3Client;

mod archive;
mod index;
mod sync;

pub use index::ActivityIndex;

pub struct ActivitySync {
    intervals_client: IntervalsClient,
    s3_client: S3Client,
    s3_bucket: String,
    athlete_id: String,
    work_dir: std::path::PathBuf,
}

impl ActivitySync {
    pub fn new(api_key: &str, athlete_id: &str, s3_client: S3Client, s3_bucket: &str, work_dir: &std::path::Path) -> Self {
        Self {
            intervals_client: IntervalsClient::new(api_key.to_string()),
            s3_client,
            s3_bucket: s3_bucket.to_string(),
            athlete_id: athlete_id.to_string(),
            work_dir: work_dir.to_path_buf(),
        }
    }
}
