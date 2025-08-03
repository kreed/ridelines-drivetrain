# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

This is a Rust CLI application for interfacing with the intervals.icu API to retrieve athlete activity data. The tool allows users to list activities and download GPX files for specific activities.

## Development Commands

### Building and Running
- `cargo build` - Build the project
- `cargo run -- --help` - Show CLI help
- `cargo run -- list -i <athlete_id>` - List activities for an athlete
- `cargo run -- download -i <activity_id> -p <output_path>` - Download GPX file for a specific activity
- `cargo run -- download-all -i <athlete_id> -o <output_dir>` - Download all GPX files for an athlete

### Environment Configuration
- **Required**: Create a `.env` file with `INTERVALS_API_KEY=your_api_key_here`
- API key is loaded from environment variable or `.env` file only (no CLI argument)
- Use `env.example` as a template for environment setup

### Testing and Quality
- `cargo test` - Run tests
- `cargo clippy` - Run linter
- `cargo fmt` - Format code

## Architecture

### Core Structure
- **CLI Interface**: Uses `clap` with subcommands for different operations
- **Commands**: 
  - `list` - Get athlete activities and display them
  - `download` - Download single activity GPX file to specified path
  - `download-all` - Bulk download all GPX files for an athlete with smart sync
- **HTTP Client**: Uses `reqwest` with retry middleware (`reqwest-retry`) for robust API calls
- **Data Format**: CSV parsing for activities list using `serde` and `csv` crate
- **Authentication**: Basic auth using base64-encoded "API_KEY:{api_key}" format

### API Integration
- **Base URL**: `https://intervals.icu`
- **Activities endpoint**: `/api/v1/athlete/{athlete_id}/activities.csv`
- **GPX download**: `/api/v1/activity/{activity_id}/gpx-file`
- **Auth header**: `Authorization: Basic {base64_encoded_credentials}`

### Data Models
- `Activity` struct captures key fields from CSV: id, name, start_date_local, distance, total_elevation_gain, trainer
- Full CSV contains extensive fields for power, heart rate, training metrics

### Download-All Features
- **Smart Sync**: Only downloads/updates activities when name, start time, or distance changes
- **Filename-based Metadata**: Uses format `{YYYY-MM-DD}-{sanitized_name}-{activity_id}-{distance}.gpx`
- **Smart GPS Detection**: Skips activities without GPS data using heuristics (no distance || trainer without elevation)
- **Progress Reporting**: Shows download progress with `indicatif` progress bar
- **Retry Logic**: Automatic retries (2x) for transient failures using `reqwest-retry`
- **Filename Sanitization**: Uses `sanitize-filename` crate for safe, cross-platform filenames
- **Cleanup**: Removes local GPX files for activities no longer present on intervals.icu
- **Statistics**: Reports downloaded, skipped (unchanged), skipped (no GPS), failed, and deleted counts

## Known Issues
- Error handling uses `.unwrap()` in some places - consider proper error handling for production use