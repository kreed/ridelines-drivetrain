use aws_config::BehaviorVersion;
use aws_lambda_events::apigw::ApiGatewayProxyResponse;
use aws_sdk_dynamodb::Client as DynamoDBClient;
use cloudfront_sign::{SignedOptions, get_signed_url};
use lambda_runtime::Error;
use ridelines_drivetrain::{
    api::UserProfileResponse,
    common::{metrics, types::User},
};
use std::env;
use tracing::info;

use crate::utils::create_json_response;

pub async fn handle_get_user_profile(user_id: String) -> Result<ApiGatewayProxyResponse, Error> {
    info!("Getting user profile for user: {}", user_id);

    let users_table =
        env::var("USERS_TABLE_NAME").map_err(|_| Error::from("USERS_TABLE_NAME not set"))?;
    let cloudfront_key_pair_id = env::var("CLOUDFRONT_KEY_PAIR_ID")
        .map_err(|_| Error::from("CLOUDFRONT_KEY_PAIR_ID not set"))?;
    let cloudfront_private_key = env::var("CLOUDFRONT_PRIVATE_KEY")
        .map_err(|_| Error::from("CLOUDFRONT_PRIVATE_KEY not set"))?;
    let frontend_url = env::var("FRONTEND_URL").map_err(|_| Error::from("FRONTEND_URL not set"))?;

    // Initialize AWS clients
    let config = aws_config::load_defaults(BehaviorVersion::latest()).await;
    let dynamodb_client = DynamoDBClient::new(&config);

    // Get user from DynamoDB
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

    // Generate CloudFront signed URL for PMTiles file
    let pmtiles_path = format!("/activities/{}.pmtiles", user.athlete_id);
    let pmtiles_url = format!("{}{}", frontend_url, pmtiles_path);
    let expires = chrono::Utc::now() + chrono::Duration::hours(1);

    // Create CloudFront signed URL
    let signed_options = SignedOptions {
        key_pair_id: cloudfront_key_pair_id.into(),
        private_key: cloudfront_private_key.into(),
        date_less_than: expires.timestamp() as u64,
        ..Default::default()
    };

    let signed_url = get_signed_url(&pmtiles_url, &signed_options)
        .map_err(|e| Error::from(format!("Failed to create CloudFront signed URL: {e}")))?;

    let pmtiles_url = signed_url;

    let response = UserProfileResponse {
        user: ridelines_drivetrain::api::UserProfile {
            id: uuid::Uuid::parse_str(&user.id)
                .map_err(|e| Error::from(format!("Invalid user ID format: {e}")))?,
            athlete_id: user.athlete_id,
            name: user.name,
            email: user.email,
            created_at: user.created_at,
            updated_at: user.updated_at,
        },
        pmtiles_url,
    };

    info!("User profile retrieved successfully");
    metrics::increment_lambda_success();

    Ok(create_json_response(200, &response))
}
