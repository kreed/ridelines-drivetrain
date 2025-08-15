use crate::metrics_helper;
use aws_sdk_s3::Client as S3Client;
use aws_sdk_s3::primitives::ByteStream;
use function_timer::time;
use rusqlite::Connection;
use tracing::info;

pub struct TileUploader {
    s3_client: S3Client,
    athlete_id: String,
}

impl TileUploader {
    pub fn new(s3_client: S3Client, athlete_id: String) -> Self {
        Self {
            s3_client,
            athlete_id,
        }
    }

    #[time("extract_and_upload_tiles_duration")]
    pub async fn extract_and_upload_tiles(
        &self,
        mbtiles_file: &str,
        _temp_tiles_dir: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
        info!(
            "Extracting tiles from MBTiles using rusqlite: {}",
            mbtiles_file
        );

        // Open the MBTiles file as SQLite database
        let conn = match Connection::open(mbtiles_file) {
            Ok(conn) => {
                metrics_helper::increment_sqlite_success();
                conn
            }
            Err(e) => {
                metrics_helper::increment_sqlite_error();
                return Err(format!("Failed to open MBTiles file: {e}").into());
            }
        };

        // Prepare statement to get all tiles
        let mut stmt = conn.prepare(
            "SELECT zoom_level, tile_column, tile_row, tile_data FROM tiles ORDER BY zoom_level, tile_column, tile_row"
        ).map_err(|e| format!("Failed to prepare SQL statement: {e}"))?;

        let tile_iter = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, i32>(0)?,     // z
                    row.get::<_, i32>(1)?,     // x
                    row.get::<_, i32>(2)?,     // y
                    row.get::<_, Vec<u8>>(3)?, // tile_data
                ))
            })
            .map_err(|e| format!("Failed to query tiles: {e}"))?;

        let mut tile_count = 0;

        // Upload each tile directly to S3
        for tile_result in tile_iter {
            let (z, x, y, tile_data) =
                tile_result.map_err(|e| format!("Failed to read tile: {e}"))?;

            // Flip Y coordinate from TMS to XYZ coordinate system
            let y_flipped = 2i32.pow(z as u32) - 1 - y;

            // Create S3 key in the standard tile server format: z/x/y.pbf
            let s3_key = format!("strava/{}/{}/{}/{}.pbf", self.athlete_id, z, x, y_flipped);

            // Upload tile data to S3
            match self
                .s3_client
                .put_object()
                .bucket("kreed.org-website")
                .key(&s3_key)
                .body(ByteStream::from(tile_data))
                .content_type("application/x-protobuf")
                .content_encoding("gzip")
                .send()
                .await
            {
                Ok(_) => metrics_helper::increment_s3_upload_success(),
                Err(e) => {
                    metrics_helper::increment_s3_upload_failure();
                    return Err(format!("Failed to upload tile {s3_key}: {e}").into());
                }
            }

            tile_count += 1;
            if tile_count % 100 == 0 {
                info!("Uploaded {} tiles", tile_count);
            }
        }

        // Record total tiles generated
        metrics_helper::set_total_tiles_generated(tile_count);

        info!("Successfully uploaded {} tiles to S3", tile_count);
        Ok(())
    }
}
