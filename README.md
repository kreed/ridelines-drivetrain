# Ridelines Drivetrain

A high-performance Rust AWS Lambda package for multi-user GPS activity processing from intervals.icu. The drivetrain provides three specialized Lambda functions: OAuth authentication, protected user APIs, and activity sync processing. Built for speed, security, and scalability with OAuth 2.0 integration.

## Overview

Ridelines Drivetrain is the backend powerhouse of the Ridelines ecosystem, built for speed, reliability, and efficiency. It handles the complete data processing pipeline from GPS activity ingestion to web-ready vector tiles, using Rust's performance advantages and AWS's serverless scale.

### Key Features

- **🔐 OAuth 2.0 Authentication**: Secure intervals.icu integration with JWT tokens
- **👥 Multi-User Support**: User-specific activity processing and PMTiles
- **🚀 High-Performance FIT Processing**: Convert Garmin FIT files to GeoJSON with gap detection
- **🧠 Smart Synchronization**: Hash-based change detection for incremental updates
- **🗺️ PMTiles Generation**: Create optimized vector tiles using Tippecanoe
- **🛡️ Protected APIs**: JWT-secured endpoints via API Gateway
- **☁️ AWS Native**: Three specialized Lambda functions with comprehensive monitoring
- **🔄 Automatic Cache Management**: CloudFront invalidation for instant updates
- **📊 4-Phase Sync Workflow**: Robust, resumable processing pipeline

## Architecture

### Lambda Functions

The drivetrain package provides three specialized Lambda functions:

| Function | Binary | Purpose | Trigger |
|----------|--------|---------|---------|
| **auth-lambda** | `auth_lambda` | OAuth flow, JWT generation | API Gateway `/auth/*` |
| **user-lambda** | `user_lambda` | Protected APIs, user profiles | API Gateway `/api/*` |
| **sync-lambda** | `sync_lambda` | Activity download, PMTiles generation (async) | Direct invocation |

### System Architecture

```
┌─────────────────┐    ┌──────────────────┐    ┌─────────────────┐
│   Frontend      │───▶│   API Gateway    │───▶│   auth-lambda   │
│   (Hub)         │    │  (JWT Authorizer)│    │  (OAuth Flow)   │
└─────────────────┘    └──────────────────┘    └─────────────────┘
                                │                        │
                                ▼                        ▼
                       ┌──────────────────┐    ┌─────────────────┐
                       │   user-lambda    │───▶│   sync-lambda   │
                       │ (Protected APIs) │    │ (FIT Processing)│
                       └──────────────────┘    └─────────────────┘
                                │                        │
                                ▼                        ▼
                       ┌──────────────────┐    ┌─────────────────┐
                       │   DynamoDB       │    │   S3 + CDN      │
                       │ (Users/State)    │    │  (PMTiles)      │
                       └──────────────────┘    └─────────────────┘
```

#### **ActivitySync Module** (`src/activity_sync/`)
- **Purpose**: Manages the complete synchronization workflow
- **Features**: Hash-based change detection, 4-phase processing, error recovery
- **Components**:
  - `sync.rs` - Main synchronization logic
  - `archive.rs` - Activity index management with binary format
  - `index.rs` - Efficient binary operations for large datasets

#### **FIT Converter** (`src/convert.rs`)
- **Purpose**: Convert FIT files to GeoJSON with GPS track processing
- **Features**: Gap detection, track splitting, data validation
- **Performance**: Streaming processing for memory efficiency

#### **Tile Generator** (`src/tile_generator.rs`) 
- **Purpose**: Generate PMTiles from GeoJSON using Tippecanoe
- **Features**: Custom layer integration, optimized settings, compression
- **Output**: Production-ready vector tiles for web mapping

#### **intervals.icu Client** (`src/common/intervals_client.rs`)
- **Purpose**: OAuth-enabled API integration with intervals.icu
- **Features**: OAuth token handling, user profile fetching, activity API
- **Security**: Access token validation, error handling

### Data Processing Flow

#### OAuth Authentication Flow
1. **🔐 Login**: User initiates OAuth with intervals.icu
2. **🔑 Token Exchange**: Exchange authorization code for access token  
3. **👤 Profile**: Fetch user profile and athlete ID
4. **🎫 JWT**: Generate signed JWT token for frontend

#### Activity Sync Flow
1. **📡 API Trigger**: User-lambda invokes sync-lambda with SyncRequest
2. **📋 Activity List**: Fetch user activities using OAuth token
3. **🔍 Change Detection**: Compare hashes to identify updates
4. **📥 FIT Download**: Concurrent download of modified activities
5. **🔄 GeoJSON Conversion**: Process FIT files with gap detection
6. **🗺️ Tile Generation**: Create user-specific PMTiles using Tippecanoe
7. **☁️ Cloud Deployment**: Upload to S3 and invalidate CloudFront

## Technology Stack

- **Language**: Rust 1.82+ for maximum performance
- **Runtime**: AWS Lambda with `provided.al2023` custom runtime
- **Build Tool**: Cargo Lambda for cross-compilation
- **Dependencies**: 
  - AWS SDK for cloud services integration
  - Tokio for async runtime
  - Serde for JSON serialization
  - Custom FIT parsing library
- **Infrastructure**: OpenTofu/Terraform for IaC

## Getting Started

### Prerequisites

- **Rust**: 1.82+ with Cargo
- **Cargo Lambda**: For AWS Lambda development
- **AWS CLI**: Configured with appropriate permissions
- **OpenTofu/Terraform**: For infrastructure management

### Development Setup

1. **Install Cargo Lambda**:
   ```bash
   cargo install cargo-lambda
   ```

2. **Install Zig** (optional, for cross-compilation if not building on Linux):
   ```bash
   # macOS with Homebrew
   brew install zig
   ```

3. **Clone and build**:
   ```bash
   git clone https://github.com/yourusername/ridelines.git
   cd ridelines/drivetrain
   cargo build
   ```

### Available Commands

| Command | Description |
|---------|-------------|
| `cargo build` | Build all Lambda functions for local development |
| `cargo build --bin sync_lambda` | Build sync Lambda only |
| `cargo build --bin auth_lambda` | Build auth Lambda only |
| `cargo build --bin user_lambda` | Build user Lambda only |
| `cargo test` | Run test suite |
| `cargo clippy` | Run Rust linter |
| `cargo fmt` | Format code |
| `cargo lambda build --release` | Build all Lambda deployment packages |
| `cargo lambda build --release --bin sync_lambda` | Build sync Lambda package |

### Local Development

```bash
# Build all Lambda functions for Lambda environment
cargo lambda build --release

# Build specific Lambda function
cargo lambda build --release --bin auth_lambda

# Start local development server (sync lambda)
cargo lambda watch --bin sync_lambda

# Run tests with coverage
cargo test -- --nocapture

# Check code quality
cargo clippy -- -D warnings
cargo fmt --check
```

## Configuration

### Environment Variables

Each Lambda function uses these environment variables (configured via infrastructure):

#### Auth Lambda
```bash
OAUTH_CREDENTIALS_SECRET_ARN=arn:aws:secretsmanager:region:account:secret:name
JWT_KMS_KEY_ID=alias/ridelines-jwt
USERS_TABLE_NAME=ridelines-users
OAUTH_STATE_TABLE_NAME=ridelines-oauth-state
FRONTEND_URL=https://ridelines.xyz
```

#### User Lambda  
```bash
USERS_TABLE_NAME=ridelines-users
JWT_KMS_KEY_ID=alias/ridelines-jwt
SYNC_LAMBDA_FUNCTION_NAME=ridelines-sync-lambda
```

#### Sync Lambda
```bash
S3_BUCKET=your-geojson-bucket
CLOUDFRONT_DISTRIBUTION_ID=YOUR_DISTRIBUTION_ID
RUST_LOG=info                    # Logging level
TIPPECANOE_ARGS="--drop-rate=0"  # Custom Tippecanoe settings
```

### intervals.icu OAuth Setup

1. **Register OAuth App**: Visit intervals.icu settings to create OAuth application
2. **Store Credentials in Secrets Manager**:
   ```bash
   aws secretsmanager create-secret \
     --name "ridelines-oauth-credentials" \
     --description "OAuth credentials for intervals.icu integration" \
     --secret-string '{"client_id":"your-client-id","client_secret":"your-client-secret"}'
   ```

## Project Structure

```
drivetrain/
├── src/
│   ├── lib.rs                     # Module declarations
│   ├── common/                    # Shared modules for all Lambda functions
│   │   ├── aws.rs                # AWS client configurations
│   │   ├── intervals_client.rs   # OAuth-enabled intervals.icu client
│   │   ├── metrics.rs            # CloudWatch metrics integration
│   │   ├── models.rs             # Shared data models
│   │   └── error.rs              # Common error types
│   ├── auth_lambda/              # OAuth authentication Lambda
│   │   └── main.rs               # OAuth login and callback handlers
│   ├── user_lambda/              # Protected user API Lambda
│   │   └── main.rs               # User profile and sync trigger APIs
│   ├── sync_lambda/              # Activity processing Lambda
│   │   ├── main.rs               # Lambda entry point
│   │   ├── activity_sync/        # Core synchronization logic
│   │   │   ├── mod.rs           # Module exports
│   │   │   ├── sync.rs          # 4-phase sync implementation
│   │   │   ├── archive.rs       # ActivityIndex binary format
│   │   │   └── index.rs         # Efficient binary operations
│   │   ├── fit_converter.rs     # FIT to GeoJSON conversion
│   │   └── tile_generator.rs    # PMTiles generation with Tippecanoe
├── tests/                        # Integration and unit tests
├── Cargo.toml                   # Multiple binary targets and dependencies
├── Cargo.lock                   # Dependency lock file
└── README.md                    # This file
```

## Performance Optimization

### Sync Efficiency
- **Hash-based Change Detection**: Only processes modified activities
- **Incremental Updates**: Preserves existing data, adds only changes
- **Concurrent Downloads**: Configurable parallelism with semaphore control
- **Binary Index Format**: Custom format for fast activity lookup

### Lambda Configuration
- **Memory**: 2048MB for optimal performance
- **Timeout**: 600 seconds (10 minutes) for large datasets
- **Runtime**: Custom Rust runtime on `provided.al2023`
- **Architecture**: ARM64 for better price/performance

### Compression & Storage
- **GeoJSON**: Zstandard compression (level 3) for archives
- **PMTiles**: Optimal compression settings via Tippecanoe
- **S3 Transfer**: Multipart upload for large files
- **CloudFront**: Efficient cache headers for global distribution

## Monitoring & Observability

### CloudWatch Metrics

The service emits comprehensive metrics:

```rust
// Example metrics emitted
- sync.activities.downloaded (Count)
- sync.activities.processed (Count)  
- sync.tiles.generated (Count)
- sync.duration.total (Duration)
- sync.s3.upload.duration (Duration)
- cloudfront.invalidation.duration (Duration)
```

### Structured Logging

```json
{
  "timestamp": "2024-01-15T10:30:00Z",
  "level": "INFO",
  "target": "drivetrain::activity_sync",
  "athlete_id": "i123",
  "phase": "download",
  "activities_processed": 15,
  "duration_ms": 2341
}
```

### Error Handling

- **Retry Logic**: Exponential backoff for transient failures
- **Graceful Degradation**: Continues processing on partial failures
- **Dead Letter Queue**: Failed events for manual investigation
- **Detailed Errors**: Structured error context for debugging

## Testing

### Test Suite

```bash
# Run all tests
cargo test

# Run with output
cargo test -- --nocapture

# Run specific test
cargo test test_fit_conversion

# Integration tests only
cargo test --test integration
```

### Test Coverage

- **Unit Tests**: Core logic and data processing
- **Integration Tests**: End-to-end workflow testing
- **Property Tests**: Edge case validation
- **Performance Tests**: Benchmark critical paths

## Deployment

### CI/CD Pipeline

The project uses GitHub Actions for automated deployment. A workflow in this repository builds a drivetrain package and then
kicks off a deployment through [ridelines-frame](https://github.com/kreed/ridelines-frame/).

## Troubleshooting

### Debug Mode

Enable verbose logging:

```bash
# Set environment variable
RUST_LOG=debug

# Or in Lambda console
RUST_LOG=drivetrain=debug,activity_sync=trace
```

### Performance Profiling

```bash
# Build with profiling
cargo build --release --features profiling

# Run with CPU profiling
CARGO_PROFILE_RELEASE_DEBUG=true cargo lambda build --release
```

## Links

- [Infrastructure (Frame)](https://github.com/kreed/ridelines-frame/)
- [Frontend (Hub)](https://github.com/kreed/ridelines-hub/)
- **intervals.icu API**: [Documentation](https://intervals.icu/api)
- **PMTiles Specification**: [GitHub](https://github.com/protomaps/PMTiles)
- **Tippecanoe**: [Tippecanoe](https://github.com/felt/tippecanoe)