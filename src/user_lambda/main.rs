use lambda_runtime::{Error, LambdaEvent};
use metrics_cloudwatch_embedded::lambda::handler::run;
use ridelines_drivetrain::common::metrics;
use serde_json::{Value, json};
use tracing::info_span;

async fn function_handler(_event: LambdaEvent<Value>) -> Result<Value, Error> {
    Ok(json!({
        "statusCode": 200,
        "headers": {
            "Content-Type": "application/json"
        },
        "body": json!({
            "message": "User Lambda placeholder - handles protected API endpoints",
            "endpoints": [
                "GET /api/user - Returns user profile and PMTiles URL",
                "POST /api/sync - Triggers activity sync for authenticated user"
            ]
        }).to_string()
    }))
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    tracing_subscriber::fmt()
        .json()
        .with_env_filter(tracing_subscriber::filter::EnvFilter::from_default_env())
        .with_target(false)
        .with_current_span(false)
        .without_time()
        .init();

    let metrics = metrics_cloudwatch_embedded::Builder::new()
        .cloudwatch_namespace(metrics::METRICS_NAMESPACE)
        .lambda_cold_start_span(info_span!("cold start"))
        .lambda_cold_start_metric("ColdStart")
        .with_lambda_request_id("RequestId")
        .init()?;

    run(metrics, function_handler).await
}
