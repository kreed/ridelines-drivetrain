use aws_config::BehaviorVersion;
use aws_lambda_events::apigw::{
    ApiGatewayCustomAuthorizerRequestTypeRequest, ApiGatewayCustomAuthorizerResponse,
    ApiGatewayCustomAuthorizerPolicy,
};
use aws_lambda_events::event::iam::{IamPolicyStatement, IamPolicyEffect};
use aws_sdk_kms::Client as KmsClient;
use lambda_runtime::{Error, LambdaEvent};
use metrics_cloudwatch_embedded::lambda::handler::run;
use ridelines_drivetrain::common::metrics;
use std::collections::HashMap;
use std::env;
use tracing::{error, info, info_span};

use ridelines_drivetrain::common::jwt::{verify_jwt_token, JwtClaims};

async fn function_handler(
    event: LambdaEvent<ApiGatewayCustomAuthorizerRequestTypeRequest>,
) -> Result<ApiGatewayCustomAuthorizerResponse, Error> {
    let (request, _context) = event.into_parts();
    
    info!("Processing API Gateway custom authorizer request");
    handle_api_gateway_authorizer(request).await
}

async fn handle_api_gateway_authorizer(
    request: ApiGatewayCustomAuthorizerRequestTypeRequest,
) -> Result<ApiGatewayCustomAuthorizerResponse, Error> {
    info!("Processing API Gateway authorization request");

    // Extract JWT from Cookie header
    let cookie_header = request
        .headers
        .get("cookie")
        .or_else(|| request.headers.get("Cookie"))
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    let jwt_token = extract_jwt_from_cookie(cookie_header).ok_or_else(|| {
        info!("No JWT token found in cookies");
        Error::from("Unauthorized")
    })?;

    // Verify JWT
    let claims = verify_jwt(jwt_token).await?;

    info!("JWT verified successfully for user: {}", claims.sub);
    metrics::increment_lambda_success();

    // Create policy document
    let policy = ApiGatewayCustomAuthorizerPolicy {
        version: Some("2012-10-17".to_string()),
        statement: vec![IamPolicyStatement {
            action: vec!["execute-api:Invoke".to_string()],
            effect: IamPolicyEffect::Allow,
            resource: vec![request.method_arn.unwrap_or_default()],
            ..Default::default()
        }],
    };

    // Create context with user information
    let mut context_map = HashMap::new();
    context_map.insert(
        "userId".to_string(),
        serde_json::Value::String(claims.sub.clone()),
    );
    context_map.insert(
        "athleteId".to_string(),
        serde_json::Value::String(claims.athlete_id.clone()),
    );
    if let Some(username) = claims.username {
        context_map.insert("username".to_string(), serde_json::Value::String(username));
    }

    Ok(ApiGatewayCustomAuthorizerResponse {
        principal_id: Some(claims.sub),
        policy_document: policy,
        context: serde_json::Value::Object(serde_json::Map::from_iter(context_map)),
        usage_identifier_key: None,
    })
}


async fn verify_jwt(token: &str) -> Result<JwtClaims, Error> {
    // Get KMS key ID from environment
    let jwt_kms_key_id = env::var("JWT_KMS_KEY_ID").map_err(|_| Error::from("JWT_KMS_KEY_ID not set"))?;

    // Initialize AWS clients
    let config = aws_config::load_defaults(BehaviorVersion::latest()).await;
    let kms_client = KmsClient::new(&config);

    // Verify JWT token
    verify_jwt_token(token, &jwt_kms_key_id, &kms_client)
        .await
        .map_err(|e| {
            error!("JWT verification failed: {}", e);
            Error::from("Unauthorized")
        })
}

fn extract_jwt_from_cookie(cookie_header: &str) -> Option<&str> {
    cookie_header
        .split(';')
        .map(|cookie| cookie.trim())
        .find(|cookie| cookie.starts_with("ridelines_auth="))
        .and_then(|cookie| cookie.strip_prefix("ridelines_auth="))
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