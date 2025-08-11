# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

This is a Rust CLI application for interfacing with the intervals.icu API to retrieve athlete activity data. The tool allows users to list activities, download FIT files, and convert them to GeoJSON format for mapping and analysis.

## Development Commands

### Building and Running
- `cargo build` - Build the project
- `cargo run -- --help` - Show CLI help
- `cargo run -- list -i <athlete_id>` - List activities for an athlete
- `cargo run -- download -i <activity_id> -p <output_path>` - Download FIT file for a specific activity
- `cargo run -- sync -i <athlete_id> -o <output_dir>` - Sync all activities as GeoJSON files (.geojson for GPS data, .stub for no GPS data) for an athlete

### Environment Configuration
- **Required**: Create a `.env` file with `INTERVALS_API_KEY=your_api_key_here`
- API key is loaded from environment variable or `.env` file only (no CLI argument)
- Use `env.example` as a template for environment setup

### Testing and Quality
- `cargo test` - Run tests
- `cargo clippy` - Run linter (should be run after code changes)
- `cargo fmt` - Format code

## Architecture

### Core Structure
- **CLI Interface**: Uses `clap` with subcommands for different operations
- **Commands**: 
  - `list` - Get athlete activities and display them
  - `download` - Download single activity FIT file to specified path
  - `sync` - Main workflow: sync all activities as GeoJSON files (.geojson for GPS data, .stub for no GPS data) for an athlete with smart sync
- **HTTP Client**: Uses `reqwest` with retry middleware (`reqwest-retry`) for robust API calls
- **Data Format**: CSV parsing for activities list using `serde` and `csv` crate
- **GeoJSON Conversion**: Automatic conversion of FIT data to GeoJSON format using `fitparser` and `geojson` crates
- **Authentication**: Basic auth using base64-encoded "API_KEY:{api_key}" format

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
- **Automatic GeoJSON Conversion**: Downloads FIT data and converts to GeoJSON format automatically (.geojson files), or creates empty stub files (.stub) for activities without GPS data
- **Filename-based Metadata**: Uses format `{YYYY-MM-DD}-{sanitized_name}-{activity_type}-{distance}-{elapsed_time}-{activity_id}.geojson` or `.stub`
- **GPS Detection**: Downloads all activities and creates .geojson files for those with GPS data, .stub files for those without
- **Progress Reporting**: Shows download progress with `indicatif` progress bar
- **Retry Logic**: Automatic retries (2x) for transient failures using `reqwest-retry`
- **Filename Sanitization**: Uses `sanitize-filename` crate for safe, cross-platform filenames
- **Cleanup**: Removes local activity files (.geojson and .stub) for activities no longer present on intervals.icu
- **Statistics**: Reports downloaded, skipped (unchanged), downloaded (empty/no GPS), failed, and deleted counts

## Known Issues
- Error handling uses `.unwrap()` in some places - consider proper error handling for production use