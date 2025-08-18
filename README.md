# Ridelines Drivetrain

A high-performance Rust AWS Lambda function for processing GPS activity data from intervals.icu. This service downloads FIT files, converts them to GeoJSON, and generates optimized PMTiles for efficient web mapping visualization.

## Features

- **Intervals.icu Integration**: Secure API integration with automatic authentication
- **Smart Sync**: Hash-based change detection with incremental updates
- **FIT Processing**: Convert FIT files to GeoJSON with GPS gap detection
- **PMTiles Generation**: Create optimized vector tiles using Tippecanoe
- **CloudFront Integration**: Automatic cache invalidation for updated tiles
- **AWS Native**: Built for Lambda with comprehensive monitoring and logging

## Architecture

### Core Components

- **ActivitySync Module**: Handles data synchronization with smart change detection
- **FIT Converter**: Processes FIT files with GPS track splitting on large gaps
- **Tile Generator**: Creates PMTiles using custom Tippecanoe layer
- **S3 Integration**: Dual-bucket architecture for data and PMTiles storage
- **CloudFront Management**: Automatic cache invalidation for updated content

### Data Flow

1. **Trigger**: EventBridge event with athlete ID
2. **Sync**: Download activity list from intervals.icu
3. **Process**: Hash-based comparison for changed activities
4. **Convert**: Download FIT files and convert to GeoJSON
5. **Generate**: Create PMTiles using Tippecanoe
6. **Deploy**: Upload to S3 and invalidate CloudFront cache

## Development

### Prerequisites

- Rust 1.88+ with Cargo Lambda
- Zig 0.14.1 (for cross-compilation)
- AWS CLI configured
- OpenTofu/Terraform

### Setup

1. **Install Cargo Lambda**:
   ```bash
   cargo install cargo-lambda
   ```

2. **Install Zig** (for cross-compilation):
   ```bash
   # macOS
   brew install zig
   
   # Or download from https://ziglang.org/
   ```

3. **Build the project**:
   ```bash
   cargo build
   ```

### Available Commands

- `cargo test` - Run tests
- `cargo clippy` - Run linter
- `cargo fmt` - Format code
- `cargo lambda build --release` - Build Lambda deployment package
- `cargo lambda deploy` - Deploy to AWS (if configured)

### Local Testing

```bash
# Build for Lambda environment
cargo lambda build --release

# Test locally with cargo lambda
cargo lambda watch
```

## Deployment

### Infrastructure Setup

The project includes complete infrastructure as code:

```bash
cd terraform
tofu init
tofu plan
tofu apply
```

### Environment Variables

Required environment variables (set via Terraform):

- `SECRETS_MANAGER_SECRET_ARN` - intervals.icu API key location
- `S3_BUCKET` - Data storage bucket for archives
- `ACTIVITIES_S3_BUCKET` - PMTiles storage bucket  
- `CLOUDFRONT_DISTRIBUTION_ID` - For cache invalidation
- `RUST_LOG` - Logging level (default: info)

### GitHub Actions

Automated deployment via GitHub Actions:

- **Lambda Build**: Builds and uploads deployment package
- **Infrastructure**: Deploys AWS resources via OpenTofu
- **Layer Management**: Builds custom Tippecanoe layer

Required repository variables:
- `ACTIVITIES_BUCKET_NAME` - From hub infrastructure output
- `CLOUDFRONT_DISTRIBUTION_ID` - From hub infrastructure output

## Configuration

### Intervals.icu API

Store your intervals.icu API key in AWS Secrets Manager:

```bash
aws secretsmanager create-secret \
  --name "ridelines-drivetrain-intervals-api-key" \
  --secret-string "your-api-key-here"
```

### Triggering Execution

Send EventBridge event with athlete ID:

```json
{
  "detail": {
    "athlete_id": "i351926"
  }
}
```

## Project Structure

```
├── src/
│   ├── main.rs                 # Lambda runtime entry point
│   ├── activity_sync/          # Core sync functionality
│   │   ├── mod.rs             # Public interface
│   │   ├── sync.rs            # 4-phase sync workflow
│   │   ├── archive.rs         # ActivityIndex management
│   │   └── index.rs           # Binary index operations
│   ├── intervals_client.rs    # HTTP client for intervals.icu
│   ├── convert.rs             # FIT to GeoJSON conversion
│   ├── tile_generator.rs      # PMTiles generation
│   └── metrics_helper.rs      # CloudWatch metrics
├── terraform/                 # Infrastructure as code
│   ├── main.tf               # Lambda function and IAM
│   └── variables.tf          # Configuration variables
├── .github/workflows/        # CI/CD pipelines
│   ├── lambda.yml           # Build and deploy
│   ├── infrastructure.yml   # Infrastructure updates
│   └── tippecanoe-layer.yml # Custom layer build
└── Cargo.toml               # Rust dependencies
```

## Performance

### Smart Sync Features

- **Hash-based change detection**: Only processes modified activities
- **Concurrent processing**: 5-concurrent FIT downloads with semaphore control
- **Streaming architecture**: Memory-efficient handling of large datasets
- **Incremental updates**: Preserves existing data, only adds/updates changes

### Resource Optimization

- **Lambda Configuration**: 2048MB memory, 10-minute timeout
- **Binary Size**: Optimized with LTO and size flags
- **Custom Layer**: Tippecanoe binaries built from source
- **Compression**: Zstandard level 3 for GeoJSON archives

## Monitoring

### CloudWatch Metrics

- Success/failure rates for all operations
- Processing counts and timing metrics
- PMTiles file sizes and compression ratios
- S3 upload performance and cache invalidation timing

### Logging

- Structured JSON logging optimized for CloudWatch
- Method-level performance measurement
- Comprehensive error tracking with context

## Contributing

1. Fork the repository
2. Create a feature branch: `git checkout -b feature-name`
3. Make your changes
4. Run tests: `cargo test`
5. Check code quality: `cargo clippy && cargo fmt --check`
6. Commit changes: `git commit -m "Description"`
7. Push to branch: `git push origin feature-name`
8. Open a Pull Request

## License

MIT License. See [LICENSE](LICENSE) for details.

This project is part of the Ridelines ecosystem for processing and visualizing GPS activity data.