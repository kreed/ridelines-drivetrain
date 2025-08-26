use aws_config::BehaviorVersion;
use aws_lambda_events::apigw::{ApiGatewayProxyRequest, ApiGatewayProxyResponse};
use aws_lambda_events::http::HeaderMap;
use aws_sdk_dynamodb::Client as DynamoDBClient;
use aws_sdk_dynamodb::types::AttributeValue;
use aws_sdk_kms::Client as KmsClient;
use aws_sdk_secretsmanager::Client as SecretsManagerClient;
use chrono::{Duration, Utc};
use lambda_runtime::{Error, LambdaEvent};
use metrics_cloudwatch_embedded::lambda::handler::run;
use ridelines_drivetrain::{
    api::CallbackQueryParams,
    common::{
        intervals_client::{IntervalsClient, OAuthTokenRequest},
        metrics,
        types::{OAuthState, User},
    },
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::env;
use tracing::{error, info, info_span};
use uuid::Uuid;

use ridelines_drivetrain::common::jwt::{JwtClaims, generate_jwt_token};

const OAUTH_AUTHORIZE_URL: &str = "https://intervals.icu/oauth/authorize";
const OAUTH_SCOPE: &str = "ACTIVITY:READ";
const STATE_TTL_SECONDS: i64 = 600; // 10 minutes

#[derive(Debug, Serialize, Deserialize)]
struct OAuthCredentials {
    client_id: String,
    client_secret: String,
}

// Using generated types from OpenAPI spec instead of manual structs

async fn function_handler(
    event: LambdaEvent<ApiGatewayProxyRequest>,
) -> Result<ApiGatewayProxyResponse, Error> {
    let (request, _context) = event.into_parts();

    let path = request
        .path
        .clone()
        .unwrap_or_else(|| "unknown".to_string());
    let method = request.http_method.to_string();

    info!("Auth Lambda invoked - path: {}, method: {}", path, method);

    match (method.as_str(), path.as_str()) {
        ("GET", "/auth/login") => handle_login(request).await,
        ("GET", "/auth/callback") => handle_callback(request).await,
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

async fn handle_login(request: ApiGatewayProxyRequest) -> Result<ApiGatewayProxyResponse, Error> {
    info!("Processing login request");

    // Parse query parameters for optional redirect path
    let redirect_path = request
        .query_string_parameters
        .first("redirect_path")
        .map(|s| s.to_string());

    // Get environment variables
    let api_domain = env::var("API_DOMAIN").map_err(|_| Error::from("API_DOMAIN not set"))?;
    let oauth_state_table = env::var("OAUTH_STATE_TABLE_NAME")
        .map_err(|_| Error::from("OAUTH_STATE_TABLE_NAME not set"))?;
    let oauth_credentials_secret_arn = env::var("OAUTH_CREDENTIALS_SECRET_ARN")
        .map_err(|_| Error::from("OAUTH_CREDENTIALS_SECRET_ARN not set"))?;

    // Initialize AWS clients
    let config = aws_config::load_defaults(BehaviorVersion::latest()).await;
    let dynamodb_client = DynamoDBClient::new(&config);
    let secrets_client = SecretsManagerClient::new(&config);

    // Get OAuth credentials from Secrets Manager
    let secret_value = secrets_client
        .get_secret_value()
        .secret_id(&oauth_credentials_secret_arn)
        .send()
        .await
        .map_err(|e| Error::from(format!("Failed to retrieve OAuth credentials: {e}")))?;

    let credentials: OAuthCredentials = serde_json::from_str(
        secret_value
            .secret_string()
            .ok_or_else(|| Error::from("OAuth credentials not found"))?,
    )
    .map_err(|e| Error::from(format!("Failed to parse OAuth credentials: {e}")))?;

    // Generate state parameter for CSRF protection
    let state = Uuid::new_v4().to_string();
    let expires_at = Utc::now() + Duration::seconds(STATE_TTL_SECONDS);

    // Store state in DynamoDB with TTL
    let oauth_state = OAuthState {
        state: state.clone(),
        created_at: Utc::now(),
        ttl: expires_at.timestamp(),
        redirect_path,
    };

    dynamodb_client
        .put_item()
        .table_name(&oauth_state_table)
        .set_item(Some(serde_dynamo::to_item(oauth_state).map_err(|e| {
            Error::from(format!("Failed to serialize OAuth state: {e}"))
        })?))
        .send()
        .await
        .map_err(|e| Error::from(format!("Failed to store OAuth state: {e}")))?;

    // Build OAuth URL
    let redirect_uri = format!("https://{api_domain}/auth/callback");
    let oauth_url = format!(
        "{}?response_type=code&client_id={}&redirect_uri={}&scope={}&state={}",
        OAUTH_AUTHORIZE_URL,
        urlencoding::encode(&credentials.client_id),
        urlencoding::encode(&redirect_uri),
        urlencoding::encode(OAUTH_SCOPE),
        urlencoding::encode(&state)
    );

    info!(
        "OAuth URL generated successfully, redirecting to: {}",
        oauth_url
    );
    metrics::increment_lambda_success();

    let mut headers = HeaderMap::new();
    headers.insert("Location", oauth_url.parse().unwrap());

    Ok(ApiGatewayProxyResponse {
        status_code: 302,
        headers,
        multi_value_headers: HeaderMap::new(),
        body: None,
        is_base64_encoded: false,
    })
}

async fn handle_callback(
    request: ApiGatewayProxyRequest,
) -> Result<ApiGatewayProxyResponse, Error> {
    info!("Processing OAuth callback");

    // Parse query parameters using generated type
    let params = CallbackQueryParams {
        code: request
            .query_string_parameters
            .first("code")
            .ok_or_else(|| Error::from("Missing code parameter"))?
            .to_string(),
        state: request
            .query_string_parameters
            .first("state")
            .ok_or_else(|| Error::from("Missing state parameter"))?
            .parse()
            .map_err(|e| Error::from(format!("Invalid UUID for state parameter: {e}")))?,
    };

    // Get environment variables
    let frontend_url =
        env::var("FRONTEND_URL").unwrap_or_else(|_| "https://ridelines.xyz".to_string());
    let api_domain = env::var("API_DOMAIN").map_err(|_| Error::from("API_DOMAIN not set"))?;
    let oauth_state_table = env::var("OAUTH_STATE_TABLE_NAME")
        .map_err(|_| Error::from("OAUTH_STATE_TABLE_NAME not set"))?;
    let users_table =
        env::var("USERS_TABLE_NAME").map_err(|_| Error::from("USERS_TABLE_NAME not set"))?;
    let oauth_credentials_secret_arn = env::var("OAUTH_CREDENTIALS_SECRET_ARN")
        .map_err(|_| Error::from("OAUTH_CREDENTIALS_SECRET_ARN not set"))?;
    let jwt_kms_key_id =
        env::var("JWT_KMS_KEY_ID").map_err(|_| Error::from("JWT_KMS_KEY_ID not set"))?;

    // Initialize AWS clients
    let config = aws_config::load_defaults(BehaviorVersion::latest()).await;
    let dynamodb_client = DynamoDBClient::new(&config);
    let secrets_client = SecretsManagerClient::new(&config);
    let kms_client = KmsClient::new(&config);

    // Verify and delete state parameter atomically (prevents reuse)
    let stored_oauth_state: OAuthState = dynamodb_client
        .delete_item()
        .table_name(&oauth_state_table)
        .key("state", AttributeValue::S(params.state.to_string()))
        .return_values(aws_sdk_dynamodb::types::ReturnValue::AllOld)
        .send()
        .await
        .map_err(|e| Error::from(format!("Failed to delete OAuth state: {e}")))?
        .attributes
        .ok_or_else(|| {
            error!("Invalid OAuth state parameter");
            Error::from("Invalid state parameter")
        })
        .and_then(|item| {
            serde_dynamo::from_item(item.clone())
                .map_err(|e| Error::from(format!("Failed to deserialize OAuth state: {e}")))
        })?;

    // Get OAuth credentials
    let secret_value = secrets_client
        .get_secret_value()
        .secret_id(&oauth_credentials_secret_arn)
        .send()
        .await
        .map_err(|e| Error::from(format!("Failed to retrieve OAuth credentials: {e}")))?;

    let credentials: OAuthCredentials = serde_json::from_str(
        secret_value
            .secret_string()
            .ok_or_else(|| Error::from("OAuth credentials not found"))?,
    )
    .map_err(|e| Error::from(format!("Failed to parse OAuth credentials: {e}")))?;

    // Exchange authorization code for access token
    let mut intervals_client = IntervalsClient::new();
    let redirect_uri = format!("{frontend_url}/auth/callback");

    let token_request = OAuthTokenRequest {
        grant_type: "authorization_code".to_string(),
        code: params.code,
        redirect_uri,
        client_id: credentials.client_id,
        client_secret: credentials.client_secret,
    };

    let token_response = intervals_client
        .exchange_oauth_code(token_request)
        .await
        .map_err(|e| Error::from(format!("Failed to exchange OAuth code: {e}")))?;

    // Set the access token and fetch user profile
    intervals_client.set_access_token(&token_response.access_token);
    let user_profile = intervals_client
        .get_user_profile()
        .await
        .map_err(|e| Error::from(format!("Failed to fetch user profile: {e}")))?;

    // Create or update user in DynamoDB
    let user_id = Uuid::new_v4();
    let now = Utc::now();

    let user = User {
        id: user_id.to_string(),
        athlete_id: user_profile.id.clone(),
        username: user_profile.username,
        email: user_profile.email,
        created_at: now,
        updated_at: now,
        last_login: now,
        intervals_access_token: token_response.access_token.clone(),
    };

    // Store user (upsert by athlete_id)
    dynamodb_client
        .put_item()
        .table_name(&users_table)
        .set_item(Some(serde_dynamo::to_item(&user).map_err(|e| {
            Error::from(format!("Failed to serialize user: {e}"))
        })?))
        .send()
        .await
        .map_err(|e| Error::from(format!("Failed to store user: {e}")))?;

    // Generate JWT token
    let jwt_claims = JwtClaims {
        sub: user.id.clone(),
        athlete_id: user.athlete_id.clone(),
        username: user.username.clone(),
        iat: Utc::now().timestamp(),
        exp: (Utc::now() + Duration::days(7)).timestamp(), // 7 day expiry
        iss: api_domain.clone(),
        aud: "ridelines-web".to_string(),
    };

    let jwt_token = generate_jwt_token(&jwt_claims, &jwt_kms_key_id, &kms_client)
        .await
        .map_err(|e| Error::from(format!("Failed to generate JWT: {e}")))?;

    info!("OAuth callback processed successfully");
    metrics::increment_lambda_success();

    // Determine redirect URL
    let redirect_url = stored_oauth_state
        .redirect_path
        .map(|path| format!("{}{}", frontend_url, path))
        .unwrap_or_else(|| format!("{}/dashboard", frontend_url));

    let mut headers = HeaderMap::new();
    headers.insert("Location", redirect_url.parse().unwrap());

    // Set JWT as HttpOnly cookie
    headers.insert(
        "Set-Cookie",
        format!(
            "ridelines_auth={}; Domain={}; Path=/; HttpOnly; Secure; SameSite=Strict; Max-Age={}",
            jwt_token,
            api_domain,
            60 * 60 * 24 * 7 // 7 days in seconds
        )
        .parse()
        .unwrap(),
    );

    info!("Redirecting to: {}", redirect_url);

    Ok(ApiGatewayProxyResponse {
        status_code: 302,
        headers,
        multi_value_headers: HeaderMap::new(),
        body: None,
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
