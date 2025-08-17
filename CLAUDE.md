# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

This is a Rust AWS Lambda function (ridelines-drivetrain) for interfacing with the intervals.icu API to retrieve athlete activity data, sync it to S3, and generate PMTiles for efficient web mapping. The function downloads FIT files, converts them to GeoJSON format, and uses Tippecanoe to create vector tiles optimized for web display.

## Development Commands

### Building and Deployment
- `cargo build` - Build the project for local development/testing
- `cargo lambda build --release` - Build optimized Lambda function package
- Build tippecanoe layer using GitHub Actions workflow: `.github/workflows/build-layer.yml`
- `tofu init` - Initialize Terraform in terraform/ directory  
- `tofu plan` - Preview infrastructure changes
- `tofu apply` - Deploy Lambda function and AWS resources
- Lambda function syncs activities to S3 and generates PMTiles when invoked via EventBridge

### Environment Configuration
- **API Key**: Stored in AWS Secrets Manager, configured via Terraform
- **S3 Buckets**: Data storage bucket for compressed archives, website bucket for PMTiles
- **Tippecanoe Layer**: Custom-built Lambda layer with tippecanoe binaries (deployed via GitHub Actions)
- **Environment Variables**: `SECRETS_MANAGER_SECRET_ARN`, `S3_BUCKET`, `RUST_LOG=info`
- **Lambda Trigger**: EventBridge event with `athlete_id` in detail field

### Testing and Quality
- `cargo test` - Run tests
- `cargo clippy` - Run linter (should be run after code changes)
- `cargo fmt` - Format code

## Architecture

### Core Structure
- **Main Lambda Function**: EventBridge-triggered handler with shared `work_dir` for all temporary files using `tempdir` crate
- **ActivitySync Module**: Modular design with separate files for sync logic, archive management, and public interface
- **Smart Sync Workflow**: Hash-based change detection with 4-phase processing pipeline
- **Temp Directory Management**: Uses `TempDir::new()` for automatic cleanup with structured subdirectories
- **HTTP Client**: Uses `reqwest` with `rustls-tls` and retry middleware (`reqwest-retry`) with 2-retry exponential backoff
- **Data Processing**: CSV parsing for activities list, FIT to GeoJSON conversion with GPS gap detection (>100m splits)
- **Vector Tile Generation**: Uses Tippecanoe to convert concatenated GeoJSON into optimized PMTiles format
- **Authentication**: Basic auth using base64-encoded "API_KEY:{api_key}" format
- **Storage Backend**: Compressed S3 archives (Zstandard level 3) with binary index files

### Module Organization
- `/src/main.rs` - Lambda runtime entry point and work directory creation
- `/src/activity_sync/` - Core sync functionality
  - `mod.rs` - Public interface and ActivitySync struct with work_dir integration
  - `sync.rs` - 4-phase sync workflow implementation
  - `archive.rs` - ActivityIndex management and S3 persistence
- `/src/intervals_client.rs` - HTTP client for intervals.icu API
- `/src/convert.rs` - FIT to GeoJSON conversion with gap detection
- `/src/tile_generator.rs` - PMTiles generation using Tippecanoe
- `/src/metrics_helper.rs` - CloudWatch metrics instrumentation

### API Integration
- **Base URL**: `https://intervals.icu`
- **Activities endpoint**: `/api/v1/athlete/{athlete_id}/activities.csv`
- **FIT download**: `/api/v1/activity/{activity_id}/fit-file`
- **Auth header**: `Authorization: Basic {base64_encoded_credentials}`

### Data Models
- **Activity Struct**: Captures key fields from CSV: `id`, `name`, `start_date_local`, `distance`, `activity_type`, `elapsed_time`
- **ActivityIndex (Current Architecture)**: 
  - `athlete_id: String`, `last_updated: String`
  - `geojson_activities: HashSet<String>` - Activities with GPS data
  - `empty_activities: HashSet<String>` - Activities without GPS data
  - **Key Format**: `{activity_id}:{activity_hash}` for efficient lookups
- **Activity Hashing**: Includes `id`, `name`, `start_date_local`, `elapsed_time`, `distance` for change detection
- Full CSV contains extensive fields for power, heart rate, training metrics

### Sync Features

#### 4-Phase Sync Workflow
1. **Index Loading**: Download existing binary ActivityIndex from S3 (`activities.index`)
2. **Smart Comparison**: Hash-based identification of unchanged vs new/changed activities using `try_copy()` method
3. **Parallel Processing**: 5-concurrent downloads to `work_dir/activities/` subdirectory
4. **Archive Finalization**: Stream existing + new activities into single concatenated GeoJSON file in work_dir

#### Advanced Capabilities
- **Hash-Based Change Detection**: Only processes activities when `id`, `name`, `start_date_local`, `elapsed_time`, or `distance` changes
- **Automatic GPS Classification**: Creates `.geojson` files for GPS data, `.stub` files for activities without GPS
- **Filename Convention**: `activity_{id}_{hash}.{geojson|stub}` with embedded metadata
- **Gap Handling**: Splits GPS tracks on >100m gaps using Haversine distance calculation
- **Concurrent Processing**: Semaphore-controlled (5 concurrent) with `buffer_unordered` streaming
- **Retry Logic**: 2-retry exponential backoff for transient failures using `reqwest-retry`
- **Comprehensive Metrics**: CloudWatch metrics for success/failure rates, processing counts, and resource usage
- **Streaming Architecture**: Archive finalization streams data to avoid memory overload
- **Automatic Cleanup**: `tempdir` crate provides automatic cleanup of work directories

### AWS Infrastructure
- **S3 Storage Pattern**: 
  - Data bucket: `athletes/{athlete_id}/activities.index` (binary), `activities.geojson.zst` (compressed)
  - Website bucket: `strava/{athlete_id}.pmtiles` for web serving
- **Compression**: Zstandard (level 3) for GeoJSON archives with compression ratio metrics
- **Lambda Configuration**: 2048MB memory, 10-minute timeout, `provided.al2023` runtime
- **Custom Lambda Layer**: Tippecanoe binaries built from source via GitHub Actions (`/opt/bin/tippecanoe`)
- **Secrets Manager**: Secure API key storage with IAM-restricted access
- **Infrastructure as Code**: Complete Terraform/OpenTofu management with SSM Parameter Store integration
- **GitHub Actions**: Automated build, test, layer creation, and deployment pipeline

### ActivitySync Architecture
- **Struct-based Design**: `ActivitySync` encapsulates sync logic with work_dir integration
- **Dependency Injection**: Accepts `IntervalsClient`, `S3Client`, bucket name, and work directory path
- **ActivityIndex Methods**: `new_empty()`, `try_copy()`, `insert_geojson()`, `insert_empty()`, `total_activities()`
- **S3 Integration**: Direct S3 operations for index and archive management
- **Error Handling**: Comprehensive error handling with structured logging via `tracing`
- **Resource Management**: Work directory path management with automatic cleanup
- **Modular Design**: Clean separation between sync logic, archive management, and public interface

### Performance & Monitoring
- **CloudWatch Metrics**: Success/failure counters, business logic metrics, resource usage gauges
- **Function Timing**: Method-level performance measurement via `#[time]` macro  
- **Structured Logging**: JSON-formatted CloudWatch logs optimized for Lambda environments
- **Compression Metrics**: Archive size and compression ratio tracking
- **Index Metrics**: Binary index size monitoring for performance optimization

### Key Dependencies
- **HTTP & AWS**: `reqwest` (0.12.22) with `rustls-tls`, `reqwest-retry` (0.7.0), AWS SDK crates
- **Data Processing**: `fitparser` (0.10.0), `geojson` (0.24), `geo` (0.30.0), `csv` (1.3.1), `serde` (1.0.219)
- **Storage**: `bincode` (1.3), `zstd` (0.13) for binary serialization and compression
- **Lambda Runtime**: `lambda_runtime` (0.14.3), `aws_lambda_events` (0.17.0)
- **Monitoring**: `metrics` (0.24.2), `metrics_cloudwatch_embedded` (0.7.0), `function-timer` (0.9.2)
- **Utilities**: `tempdir` (0.3), `anyhow` (1.0), `tracing` (0.1.41), `chrono` (0.4)
- **Async Runtime**: `tokio` (1.47) with `rt-multi-thread`, `macros`, `net`, `time`, `fs` features

## Known Issues
- Package size optimization achieved through rustls-tls, LTO, and size optimization flags
- Build process requires Zig toolchain for cross-compilation to Lambda runtime environment