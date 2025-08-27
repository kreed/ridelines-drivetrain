use aws_config::BehaviorVersion;
use aws_lambda_events::apigw::ApiGatewayProxyResponse;
use aws_sdk_dynamodb::Client as DynamoDBClient;
use lambda_runtime::Error;
use ridelines_drivetrain::{
    api::SyncStatusResponse,
    common::{metrics, types::User},
};
use std::env;
use tracing::info;

use crate::utils::create_json_response_with_headers;

pub async fn handle_get_sync_status(
    user_id: String,
    request_headers: &aws_lambda_events::http::HeaderMap,
) -> Result<ApiGatewayProxyResponse, Error> {
    info!("Getting sync status for user: {}", user_id);

    let users_table =
        env::var("USERS_TABLE_NAME").map_err(|_| Error::from("USERS_TABLE_NAME not set"))?;

    // Initialize AWS clients
    let config = aws_config::load_defaults(BehaviorVersion::latest()).await;
    let dynamodb_client = DynamoDBClient::new(&config);

    // Get user from DynamoDB to check sync status
    let user: User = dynamodb_client
        .get_item()
        .table_name(&users_table)
        .key(
            "id",
            aws_sdk_dynamodb::types::AttributeValue::S(user_id.clone()),
        )
        .send()
        .await
        .map_err(|e| Error::from(format!("Failed to get user from DynamoDB: {e}")))?
        .item()
        .ok_or_else(|| Error::from("User not found"))
        .and_then(|item| {
            serde_dynamo::from_item(item.clone())
                .map_err(|e| Error::from(format!("Failed to deserialize user: {e}")))
        })?;

    // For now, return a simple status based on user record
    // TODO: In the future, this could be enhanced with actual sync tracking in DynamoDB
    let response = SyncStatusResponse {
        status: ridelines_drivetrain::api::SyncStatusResponseStatus::Idle,
        last_sync_at: user.updated_at,
        activities_count: None,
        error_message: None,
    };

    info!("Sync status retrieved successfully");
    metrics::increment_lambda_success();

    Ok(create_json_response_with_headers(
        200,
        &response,
        request_headers,
    ))
}
