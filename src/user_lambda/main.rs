use aws_lambda_events::apigw::{ApiGatewayProxyRequest, ApiGatewayProxyResponse};
use lambda_runtime::{Error, LambdaEvent};
use metrics_cloudwatch_embedded::lambda::handler::run;
use ridelines_drivetrain::common::metrics;
use tracing::{error, info, info_span};

mod sync;
mod user;
mod utils;

use sync::handle_get_sync_status;
use user::handle_get_user_profile;
use utils::{create_error_response, extract_user_id_from_context};

async fn function_handler(
    event: LambdaEvent<ApiGatewayProxyRequest>,
) -> Result<ApiGatewayProxyResponse, Error> {
    let (request, _context) = event.into_parts();

    let path = request
        .path
        .clone()
        .unwrap_or_else(|| "unknown".to_string());
    let method = request.http_method.to_string();

    info!("User Lambda invoked - path: {}, method: {}", path, method);

    // Extract user ID from JWT claims (added by API Gateway JWT authorizer)
    let user_id = match extract_user_id_from_context(&request) {
        Ok(id) => id,
        Err(e) => {
            error!("Failed to extract user ID: {}", e);
            metrics::increment_lambda_failure();
            return Ok(create_error_response(401, "Invalid or missing authentication"));
        }
    };

    let result = match (method.as_str(), path.as_str()) {
        ("GET", "/api/user") => handle_get_user_profile(user_id).await,
        ("GET", "/api/sync/status") => handle_get_sync_status(user_id).await,
        _ => {
            error!("Unknown route: {} {}", method, path);
            metrics::increment_lambda_failure();
            Ok(create_error_response(404, "Not found"))
        }
    };

    // Handle any errors from the endpoint handlers
    match result {
        Ok(response) => Ok(response),
        Err(e) => {
            error!("Endpoint handler error: {}", e);
            metrics::increment_lambda_failure();
            Ok(create_error_response(500, "Internal server error"))
        }
    }
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
