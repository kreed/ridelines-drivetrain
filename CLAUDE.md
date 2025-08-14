# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

This is a Rust AWS Lambda function for interfacing with the intervals.icu API to retrieve athlete activity data and sync it to S3. The function downloads FIT files and converts them to GeoJSON format for mapping and analysis.

## Development Commands

### Building and Deployment
- `cargo build` - Build the project for local development/testing
- `cargo lambda build --release` - Build optimized Lambda function package
- `tofu init` - Initialize Terraform in terraform/ directory  
- `tofu plan` - Preview infrastructure changes
- `tofu apply` - Deploy Lambda function and AWS resources
- Lambda function syncs activities directly to S3 bucket when invoked via EventBridge

### Environment Configuration
- **API Key**: Stored in AWS Secrets Manager, configured via Terraform
- **S3 Bucket**: Auto-created by Terraform for storing GeoJSON files
- **Environment Variables**: `SECRETS_MANAGER_SECRET_ARN`, `S3_BUCKET`, `RUST_LOG=info`
- **Lambda Trigger**: EventBridge event with `athlete_id` in detail field

### Testing and Quality
- `cargo test` - Run tests
- `cargo clippy` - Run linter (should be run after code changes)
- `cargo fmt` - Format code

## Architecture

### Core Structure
- **Lambda Handler**: EventBridge-triggered function for syncing athlete activities to S3
- **Sync Workflow**: Downloads all activities as GeoJSON files (.geojson for GPS data, .stub for no GPS data) with smart sync capabilities
- **HTTP Client**: Uses `reqwest` with `rustls-tls` and retry middleware (`reqwest-retry`) for robust API calls
- **Data Format**: CSV parsing for activities list using `serde` and `csv` crate
- **GeoJSON Conversion**: Automatic conversion of FIT data to GeoJSON format using `fitparser`, `geojson`, and `geo` crates with gap detection (splits linestrings on gaps >100m)
- **Authentication**: Basic auth using base64-encoded "API_KEY:{api_key}" format
- **Storage Backend**: S3 with structured file naming and organization

### API Integration
- **Base URL**: `https://intervals.icu`
- **Activities endpoint**: `/api/v1/athlete/{athlete_id}/activities.csv`
- **FIT download**: `/api/v1/activity/{activity_id}/fit-file`
- **Auth header**: `Authorization: Basic {base64_encoded_credentials}`

### Data Models
- `Activity` struct captures key fields from CSV: id, name, start_date_local, distance, total_elevation_gain, trainer
- Full CSV contains extensive fields for power, heart rate, training metrics

### Sync Features
- **Smart Sync**: Only downloads/updates activities when name, start time, or distance changes
- **Automatic GeoJSON Conversion**: Downloads FIT data and converts to GeoJSON format automatically (.geojson files), or creates empty stub files (.stub) for activities without GPS data. Automatically splits tracks on gaps >100m to handle GPS interruptions
- **Filename-based Metadata**: Uses format `{YYYY-MM-DD}-{sanitized_name}-{activity_type}-{distance}-{elapsed_time}-{activity_id}.geojson` or `.stub`
- **GPS Detection**: Downloads all activities and creates .geojson files for those with GPS data, .stub files for those without
- **Progress Reporting**: Uses `tracing` for structured logging (optimized for Lambda environments)
- **Retry Logic**: Automatic retries (2x) for transient failures using `reqwest-retry`
- **Filename Sanitization**: Uses `sanitize-filename` crate for safe, cross-platform filenames
- **Cleanup**: Removes orphaned activity files (.geojson and .stub) for activities no longer present on intervals.icu
- **Statistics**: Reports downloaded, skipped (unchanged), downloaded (empty/no GPS), failed, and deleted counts
- **File Concatenation**: Creates `athletes/{athlete_id}/all-activities.dat` containing all GeoJSON files concatenated together
- **Concurrent Processing**: Uses semaphore-controlled concurrency (5 concurrent downloads) with async/await

### AWS Infrastructure
- **S3 Storage**: Activities stored as individual .geojson/.stub files plus concatenated .dat file
- **Secrets Manager**: Secure API key storage with IAM-restricted access
- **Lambda Configuration**: 2048MB memory, 10-minute timeout, optimized binary (4.6MB)
- **Terraform Management**: Complete infrastructure as code with state management
- **GitHub Actions**: Automated build, test, and deployment pipeline

### SyncJob Architecture
- **Struct-based Design**: `SyncJob` encapsulates all sync logic with dependency injection
- **S3 Integration**: Direct S3 operations with paginated listing and batch operations
- **Error Handling**: Comprehensive error handling with structured logging
- **Resource Management**: Arc/Mutex for shared state, Semaphore for concurrency control
- **Modular Methods**: Clean separation of concerns with focused helper methods

## Known Issues
- Package size optimization achieved through rustls-tls, LTO, and size optimization flags
- Build process requires Zig toolchain for cross-compilation to Lambda runtime environment