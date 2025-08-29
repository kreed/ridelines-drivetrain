use aws_lambda_events::apigw::{
    ApiGatewayCustomAuthorizerPolicy, ApiGatewayCustomAuthorizerRequestTypeRequest,
    ApiGatewayCustomAuthorizerResponse,
};
use aws_lambda_events::event::iam::{IamPolicyEffect, IamPolicyStatement};
use clerk_rs::apis::configuration::{ApiKey, ClerkConfiguration};
use clerk_rs::clerk::Clerk;
use clerk_rs::validators::authorizer::{ClerkJwt, validate_jwt};
use clerk_rs::validators::jwks::MemoryCacheJwksProvider;
use lambda_runtime::{Error, LambdaEvent};
use metrics_cloudwatch_embedded::lambda::handler::run;
use ridelines_drivetrain::common::metrics;
use std::collections::HashMap;
use std::env;
use std::sync::Arc;
use tracing::{error, info, info_span};

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

    // Extract JWT from Authorization header (Bearer token)
    let auth_header = request
        .headers
        .get("authorization")
        .or_else(|| request.headers.get("Authorization"))
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| {
            info!("No Authorization header found");
            Error::from("Unauthorized")
        })?;

    let jwt_token = auth_header.strip_prefix("Bearer ").ok_or_else(|| {
        info!("Authorization header missing Bearer prefix");
        Error::from("Unauthorized")
    })?;

    // Verify Clerk JWT
    let claims = verify_clerk_jwt(jwt_token).await?;

    info!("Clerk JWT verified successfully for user: {}", claims.sub);
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

    Ok(ApiGatewayCustomAuthorizerResponse {
        principal_id: Some(claims.sub),
        policy_document: policy,
        context: serde_json::Value::Object(serde_json::Map::from_iter(context_map)),
        usage_identifier_key: None,
    })
}

async fn verify_clerk_jwt(token: &str) -> Result<ClerkJwt, Error> {
    // Get Clerk publishable key from environment to construct JWKS URL
    let clerk_secret_key =
        env::var("CLERK_SECRET_KEY").map_err(|_| Error::from("CLERK_SECRET_KEY not set"))?;

    let api_key = ApiKey {
        prefix: None,
        key: clerk_secret_key,
    };
    let config = ClerkConfiguration::new(None, None, None, Some(api_key));
    let client = Clerk::new(config);

    // Create JWKS provider
    let jwks = Arc::new(MemoryCacheJwksProvider::new(client));

    // Validate the JWT using clerk-rs
    let clerk_jwt = validate_jwt(token, jwks).await.map_err(|e| {
        error!("Clerk JWT verification failed: {}", e);
        Error::from("Unauthorized")
    })?;

    info!("Clerk JWT validated for user: {}", clerk_jwt.sub);
    Ok(clerk_jwt)
}

// Cookie extraction no longer needed - using Authorization header with Clerk

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
