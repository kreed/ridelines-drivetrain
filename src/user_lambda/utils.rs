use aws_lambda_events::apigw::{ApiGatewayProxyRequest, ApiGatewayProxyResponse};
use aws_lambda_events::http::HeaderMap;
use lambda_runtime::Error;
use serde_json::json;
use tracing::error;

pub fn extract_user_id_from_context(request: &ApiGatewayProxyRequest) -> Result<String, Error> {
    // API Gateway JWT authorizer adds claims to request context
    let jwt_claims = &request
        .request_context
        .authorizer
        .jwt
        .as_ref()
        .ok_or_else(|| Error::from("Missing JWT authorizer context"))?
        .claims;

    let user_id = jwt_claims
        .get("sub")
        .ok_or_else(|| Error::from("Missing 'sub' claim in JWT"))?;

    Ok(user_id.to_string())
}

pub fn create_json_response<T: serde::Serialize>(
    status_code: i64,
    body: &T,
) -> ApiGatewayProxyResponse {
    let mut headers = HeaderMap::new();
    headers.insert("Content-Type", "application/json".parse().unwrap());

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

pub fn create_error_response(status_code: i64, message: &str) -> ApiGatewayProxyResponse {
    let error_body = json!({
        "error": message
    });

    create_json_response(status_code, &error_body)
}