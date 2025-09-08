use aws_sdk_s3::Client as S3Client;
use ridelines_drivetrain::common::intervals_client::IntervalsClient;
use std::sync::Arc;

mod archive;
mod index;
mod sync;

use crate::sync_status::SyncStatusUpdater;
pub use index::ActivityIndex;

pub struct ActivitySync {
    intervals_client: IntervalsClient,
    s3_client: S3Client,
    s3_bucket: String,
    user_id: String,
    work_dir: std::path::PathBuf,
    sync_status: Arc<SyncStatusUpdater>,
}

impl ActivitySync {
    pub fn new(
        intervals_client: IntervalsClient,
        user_id: &str,
        s3_client: S3Client,
        s3_bucket: &str,
        work_dir: &std::path::Path,
        sync_status: Arc<SyncStatusUpdater>,
    ) -> Self {
        Self {
            intervals_client,
            s3_client,
            s3_bucket: s3_bucket.to_string(),
            user_id: user_id.to_string(),
            work_dir: work_dir.to_path_buf(),
            sync_status,
        }
    }
}
