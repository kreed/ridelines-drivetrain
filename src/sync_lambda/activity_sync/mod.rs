use aws_sdk_s3::Client as S3Client;
use ridelines_drivetrain::common::intervals_client::IntervalsClient;

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
    pub fn new(
        intervals_client: IntervalsClient,
        athlete_id: &str,
        s3_client: S3Client,
        s3_bucket: &str,
        work_dir: &std::path::Path,
    ) -> Self {
        Self {
            intervals_client,
            s3_client,
            s3_bucket: s3_bucket.to_string(),
            athlete_id: athlete_id.to_string(),
            work_dir: work_dir.to_path_buf(),
        }
    }
}
