# Ridelines Drivetrain

A high-performance Rust AWS Lambda service for processing GPS activity data from intervals.icu. The drivetrain efficiently converts FIT files to GeoJSON, generates optimized PMTiles, and manages the complete data processing pipeline with smart synchronization and cloud-native architecture.

## Overview

Ridelines Drivetrain is the backend powerhouse of the Ridelines ecosystem, built for speed, reliability, and efficiency. It handles the complete data processing pipeline from GPS activity ingestion to web-ready vector tiles, using Rust's performance advantages and AWS's serverless scale.

### Key Features

- **🚀 High-Performance FIT Processing**: Convert Garmin FIT files to GeoJSON with gap detection
- **🧠 Smart Synchronization**: Hash-based change detection for incremental updates
- **🗺️ PMTiles Generation**: Create optimized vector tiles using Tippecanoe
- **☁️ AWS Native**: Built for Lambda with comprehensive monitoring
- **🔄 Automatic Cache Management**: CloudFront invalidation for instant updates
- **📊 4-Phase Sync Workflow**: Robust, resumable processing pipeline
- **⚡ Concurrent Processing**: Optimized for handling large activity datasets

## Architecture

### Core Components

```
┌─────────────────┐    ┌──────────────────┐    ┌─────────────────┐
│   EventBridge   │───▶│  Lambda Function │───▶│   S3 Storage    │
│   (Trigger)     │    │  (Rust Runtime)  │    │  (PMTiles)      │
└─────────────────┘    └──────────────────┘    └─────────────────┘
                                │
                                ▼
                       ┌──────────────────┐    ┌─────────────────┐
                       │ intervals.icu API│───▶│   CloudFront    │
                       │   (Data Source)  │    │  (Distribution) │
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

#### **intervals.icu Client** (`src/intervals_client.rs`)
- **Purpose**: Secure API integration with intervals.icu
- **Features**: Authentication, rate limiting, error handling
- **Security**: AWS Secrets Manager integration for API keys

### Data Processing Flow

1. **📡 Event Trigger**: EventBridge event with athlete ID
2. **📋 Activity List**: Fetch from intervals.icu API
3. **🔍 Change Detection**: Compare hashes to identify updates
4. **📥 FIT Download**: Concurrent download of modified activities
5. **🔄 GeoJSON Conversion**: Process FIT files with gap detection
6. **🗺️ Tile Generation**: Create PMTiles using Tippecanoe
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
- **Zig**: 0.14.1+ for cross-compilation to Linux
- **AWS CLI**: Configured with appropriate permissions
- **OpenTofu/Terraform**: For infrastructure management

### Development Setup

1. **Install Cargo Lambda**:
   ```bash
   cargo install cargo-lambda
   ```

2. **Install Zig** (required for cross-compilation):
   ```bash
   # macOS with Homebrew
   brew install zig
   
   # Linux/Windows: Download from https://ziglang.org/
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
| `cargo build` | Build for local development |
| `cargo test` | Run test suite |
| `cargo clippy` | Run Rust linter |
| `cargo fmt` | Format code |
| `cargo lambda build --release` | Build Lambda deployment package |
| `cargo lambda watch` | Local development with hot reload |

### Local Development

```bash
# Build for Lambda environment
cargo lambda build --release

# Start local development server
cargo lambda watch

# Run tests with coverage
cargo test -- --nocapture

# Check code quality
cargo clippy -- -D warnings
cargo fmt --check
```

## Configuration

### Environment Variables

The Lambda function uses these environment variables (configured via infrastructure):

```bash
# Required
INTERVALS_ICU_API_KEY_SECRET_ARN=arn:aws:secretsmanager:region:account:secret:name
ACTIVITIES_S3_BUCKET_NAME=your-activities-bucket
ATHLETE_STATE_S3_BUCKET_NAME=your-state-bucket  
CLOUDFRONT_DISTRIBUTION_ID=YOUR_DISTRIBUTION_ID

# Optional
RUST_LOG=info                    # Logging level
TIPPECANOE_ARGS="--drop-rate=0"  # Custom Tippecanoe settings
```

### intervals.icu API Setup

1. **Generate API Key**: Visit intervals.icu settings to create an API key
2. **Store in Secrets Manager**:
   ```bash
   aws secretsmanager create-secret \
     --name "ridelines-intervals-api-key" \
     --description "API key for intervals.icu integration" \
     --secret-string "your-api-key-here"
   ```

### Event Trigger Format

Trigger the Lambda function via EventBridge with this event structure:

```json
{
  "source": "ridelines.sync",
  "detail-type": "Activity Sync Request",
  "detail": {
    "athlete_id": "i351926",
    "force_sync": false
  }
}
```

## Project Structure

```
drivetrain/
├── src/
│   ├── main.rs                    # Lambda entry point and runtime
│   ├── activity_sync/             # Core synchronization logic
│   │   ├── mod.rs                # Module exports
│   │   ├── sync.rs               # 4-phase sync implementation
│   │   ├── archive.rs            # ActivityIndex binary format
│   │   └── index.rs              # Efficient binary operations
│   ├── intervals_client.rs       # intervals.icu API client
│   ├── convert.rs                # FIT to GeoJSON conversion
│   ├── tile_generator.rs         # PMTiles generation with Tippecanoe
│   └── metrics_helper.rs         # CloudWatch metrics integration
├── tests/                        # Integration and unit tests
├── terraform/                    # Infrastructure as Code
│   ├── main.tf                  # Lambda function and permissions
│   ├── variables.tf             # Configuration variables
│   └── outputs.tf               # Resource outputs
├── .github/workflows/            # CI/CD automation
│   ├── build-and-test.yml      # Test and lint on PR
│   ├── deploy-lambda.yml       # Deploy to AWS
│   └── build-layer.yml         # Tippecanoe layer build
├── Cargo.toml                   # Rust dependencies and config
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
  "athlete_id": "i351926",
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

### Infrastructure Deployment

```bash
cd terraform
tofu init
tofu plan -var="api_key_secret_name=your-secret-name"
tofu apply
```

### CI/CD Pipeline

The project uses GitHub Actions for automated deployment:

1. **Pull Request**: Run tests and linting
2. **Main Branch**: Build and deploy Lambda package
3. **Release**: Deploy infrastructure updates

### Manual Deployment

```bash
# Build release package
cargo lambda build --release

# Deploy with AWS CLI
aws lambda update-function-code \
  --function-name ridelines-drivetrain \
  --zip-file fileb://target/lambda/bootstrap/bootstrap.zip
```

## Troubleshooting

### Common Issues

1. **Build Failures**: Ensure Zig is installed for cross-compilation
2. **Memory Errors**: Increase Lambda memory if processing large datasets
3. **API Rate Limits**: Check intervals.icu API quotas and implement backoff
4. **S3 Permissions**: Verify Lambda execution role has bucket access

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

## Contributing

We welcome contributions! Please follow these guidelines:

### Development Process

1. **Fork** the repository
2. **Create** a feature branch: `git checkout -b feature/awesome-feature`
3. **Write** tests for new functionality
4. **Run** the test suite: `cargo test`
5. **Lint** the code: `cargo clippy`
6. **Format** the code: `cargo fmt`
7. **Commit** changes: `git commit -m 'Add awesome feature'`
8. **Push** to branch: `git push origin feature/awesome-feature`
9. **Open** a Pull Request

### Code Standards

- **Rust Style**: Follow `rustfmt` defaults
- **Error Handling**: Use `Result` types, avoid `panic!`
- **Documentation**: Add doc comments for public APIs
- **Testing**: Write tests for new functionality
- **Performance**: Consider memory usage and execution time

## Security

- **API Keys**: Stored in AWS Secrets Manager, never in code
- **IAM Roles**: Principle of least privilege
- **Network**: VPC isolation for sensitive operations
- **Input Validation**: All external data validated and sanitized
- **Audit Logs**: Comprehensive CloudWatch logging

## License

This project is licensed under the MIT License - see the [LICENSE](../LICENSE) file for details.

## Links

- [Infrastructure (Frame)](https://github.com/kreed/ridelines-frame/)
- [Frontend (Hub)](https://github.com/kreed/ridelines-hub/)
- **intervals.icu API**: [Documentation](https://intervals.icu/api)
- **PMTiles Specification**: [GitHub](https://github.com/protomaps/PMTiles)
- **Tippecanoe**: [Tippecanoe](https://github.com/felt/tippecanoe)