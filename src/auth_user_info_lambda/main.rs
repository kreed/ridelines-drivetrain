use aws_lambda_events::apigw::{ApiGatewayProxyRequest, ApiGatewayProxyResponse};
use aws_lambda_events::http::HeaderMap;
use lambda_runtime::{Error, LambdaEvent};
use metrics_cloudwatch_embedded::lambda::handler::run;
use ridelines_drivetrain::{
    api::UserClaims,
    common::{intervals_client::IntervalsClient, metrics},
};
use serde_json::json;
use tracing::{error, info, info_span};

async fn function_handler(
    event: LambdaEvent<ApiGatewayProxyRequest>,
) -> Result<ApiGatewayProxyResponse, Error> {
    let (request, _context) = event.into_parts();

    let path = request
        .path
        .clone()
        .unwrap_or_else(|| "unknown".to_string());
    let method = request.http_method.to_string();

    info!(
        "Auth User Info Lambda invoked - path: {}, method: {}",
        path, method
    );

    match (method.as_str(), path.as_str()) {
        ("GET", "/auth/user-info") => handle_user_info(request).await,
        _ => {
            error!("Unknown route: {} {}", method, path);
            Ok(ApiGatewayProxyResponse {
                status_code: 404,
                headers: HeaderMap::new(),
                multi_value_headers: HeaderMap::new(),
                body: Some(
                    json!({
                        "error": "Not found"
                    })
                    .to_string()
                    .into(),
                ),
                is_base64_encoded: false,
            })
        }
    }
}

async fn handle_user_info(
    request: ApiGatewayProxyRequest,
) -> Result<ApiGatewayProxyResponse, Error> {
    info!("Processing user info request");

    // Extract Authorization header
    let authorization_header = request
        .headers
        .get("authorization")
        .or_else(|| request.headers.get("Authorization"))
        .ok_or_else(|| Error::from("Missing Authorization header"))?
        .to_str()
        .map_err(|_| Error::from("Invalid Authorization header"))?;

    // Extract Bearer token
    let access_token = authorization_header
        .strip_prefix("Bearer ")
        .ok_or_else(|| Error::from("Authorization header must start with 'Bearer '"))?;

    info!("Extracted access token from Authorization header");

    // Create intervals client with access token
    let mut intervals_client = IntervalsClient::new();
    intervals_client.set_access_token(access_token);

    // Fetch user profile from intervals.icu
    let user_profile = intervals_client
        .get_user_profile()
        .await
        .map_err(|e| Error::from(format!("Failed to fetch user profile: {e}")))?;

    info!("Successfully fetched user profile from intervals.icu");

    // Transform to user claims format
    let user_claims = UserClaims {
        sub: user_profile.id,
        email: user_profile.email,
        name: user_profile.name,
        picture: user_profile.profile_medium,
        city: user_profile.city,
        state: user_profile.state,
        country: user_profile.country,
        timezone: user_profile.timezone,
        sex: user_profile.sex,
        bio: user_profile.bio,
        website: user_profile.website,
    };

    info!("User info processed successfully");
    metrics::increment_lambda_success();

    let response_body = serde_json::to_string(&user_claims)
        .map_err(|e| Error::from(format!("Failed to serialize user claims: {e}")))?;

    let mut headers = HeaderMap::new();
    headers.insert("Content-Type", "application/json".parse().unwrap());

    // Add CORS headers for Clerk
    headers.insert("Access-Control-Allow-Origin", "*".parse().unwrap());
    headers.insert(
        "Access-Control-Allow-Methods",
        "GET, OPTIONS".parse().unwrap(),
    );
    headers.insert(
        "Access-Control-Allow-Headers",
        "Authorization, Content-Type".parse().unwrap(),
    );

    Ok(ApiGatewayProxyResponse {
        status_code: 200,
        headers,
        multi_value_headers: HeaderMap::new(),
        body: Some(response_body.into()),
        is_base64_encoded: false,
    })
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
