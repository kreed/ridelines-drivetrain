use aws_lambda_events::apigw::{ApiGatewayProxyRequest, ApiGatewayProxyResponse};
use aws_lambda_events::http::HeaderMap;
use lambda_runtime::Error;
use serde_json::json;
use tracing::error;

pub fn extract_user_id_from_context(request: &ApiGatewayProxyRequest) -> Result<String, Error> {
    request
        .request_context
        .authorizer
        .fields
        .get("userId")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| Error::from("Missing or invalid 'userId' in authorizer context"))
}

fn get_cors_origin(request_headers: &HeaderMap) -> String {
    // Check Origin header first, then Referer as fallback
    let origin = request_headers
        .get("Origin")
        .or_else(|| request_headers.get("origin"))
        .and_then(|v| v.to_str().ok());

    if let Some(origin_str) = origin {
        // Allow localhost origins for development
        if origin_str.starts_with("http://localhost:")
            || origin_str.starts_with("https://localhost:")
        {
            return origin_str.to_string();
        }
    }

    // Default to frontend URL from environment
    std::env::var("FRONTEND_URL").unwrap_or_else(|_| "https://ridelines.xyz".to_string())
}

pub fn create_json_response_with_headers<T: serde::Serialize>(
    status_code: i64,
    body: &T,
    request_headers: &HeaderMap,
) -> ApiGatewayProxyResponse {
    let mut headers = HeaderMap::new();
    headers.insert("Content-Type", "application/json".parse().unwrap());

    // Set CORS headers
    let origin = get_cors_origin(request_headers);
    headers.insert("Access-Control-Allow-Origin", origin.parse().unwrap());
    headers.insert("Access-Control-Allow-Credentials", "true".parse().unwrap());

    ApiGatewayProxyResponse {
        status_code,
        headers,
        multi_value_headers: HeaderMap::new(),
        body: Some(
            serde_json::to_string(body)
                .unwrap_or_else(|e| {
                    error!("Failed to serialize response: {}", e);
                    "{}".to_string()
                })
                .into(),
        ),
        is_base64_encoded: false,
    }
}

pub fn create_error_response_with_headers(
    status_code: i64,
    message: &str,
    request_headers: &HeaderMap,
) -> ApiGatewayProxyResponse {
    let error_body = json!({
        "error": message
    });

    create_json_response_with_headers(status_code, &error_body, request_headers)
}
