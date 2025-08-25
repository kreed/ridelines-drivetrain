use aws_sdk_kms::Client as KmsClient;
use aws_sdk_kms::primitives::Blob;
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Debug, Serialize, Deserialize)]
pub struct JwtClaims {
    pub sub: String,              // User ID
    pub athlete_id: String,       // intervals.icu athlete ID
    pub username: Option<String>, // Username
    pub iat: i64,                 // Issued at timestamp
    pub exp: i64,                 // Expiry timestamp
    pub iss: String,              // Issuer (api.ridelines.xyz)
    pub aud: String,              // Audience (ridelines-web)
}

#[derive(Debug, Serialize, Deserialize)]
struct JwtHeader {
    alg: String,
    typ: String,
    kid: String, // Key ID (KMS key alias)
}

pub async fn generate_jwt_token(
    claims: &JwtClaims,
    kms_key_id: &str,
    kms_client: &KmsClient,
) -> Result<String, Box<dyn std::error::Error>> {
    // Create JWT header
    let header = JwtHeader {
        alg: "RS256".to_string(),
        typ: "JWT".to_string(),
        kid: kms_key_id.to_string(),
    };

    // Encode header and claims
    let header_json = serde_json::to_string(&header)?;
    let claims_json = serde_json::to_string(claims)?;

    let header_b64 = URL_SAFE_NO_PAD.encode(header_json.as_bytes());
    let claims_b64 = URL_SAFE_NO_PAD.encode(claims_json.as_bytes());

    // Create the message to sign
    let message = format!("{header_b64}.{claims_b64}");

    // Hash the message with SHA256
    let mut hasher = Sha256::new();
    hasher.update(message.as_bytes());
    let hash = hasher.finalize();

    // Sign with KMS
    let sign_response = kms_client
        .sign()
        .key_id(kms_key_id)
        .message(Blob::new(hash.to_vec()))
        .signing_algorithm(aws_sdk_kms::types::SigningAlgorithmSpec::RsassaPkcs1V15Sha256)
        .message_type(aws_sdk_kms::types::MessageType::Digest)
        .send()
        .await?;

    let signature = sign_response
        .signature()
        .ok_or("No signature returned from KMS")?;

    // Encode the signature
    let signature_b64 = URL_SAFE_NO_PAD.encode(signature.as_ref());

    // Construct the JWT
    Ok(format!("{message}.{signature_b64}"))
}

pub async fn verify_jwt_token(
    token: &str,
    kms_key_id: &str,
    kms_client: &KmsClient,
) -> Result<JwtClaims, Box<dyn std::error::Error>> {
    // Split JWT into parts
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() != 3 {
        return Err("Invalid JWT format".into());
    }

    let header_b64 = parts[0];
    let claims_b64 = parts[1];
    let signature_b64 = parts[2];

    // Decode and parse header
    let header_bytes = URL_SAFE_NO_PAD.decode(header_b64)?;
    let header: JwtHeader = serde_json::from_slice(&header_bytes)?;

    // Verify the key ID matches
    if header.kid != kms_key_id {
        return Err("Invalid key ID in JWT header".into());
    }

    // Decode and parse claims
    let claims_bytes = URL_SAFE_NO_PAD.decode(claims_b64)?;
    let claims: JwtClaims = serde_json::from_slice(&claims_bytes)?;

    // Check expiration
    let now = chrono::Utc::now().timestamp();
    if claims.exp < now {
        return Err("JWT token has expired".into());
    }

    // Create the message to verify
    let message = format!("{header_b64}.{claims_b64}");

    // Hash the message with SHA256
    let mut hasher = Sha256::new();
    hasher.update(message.as_bytes());
    let hash = hasher.finalize();

    // Decode the signature
    let signature = URL_SAFE_NO_PAD.decode(signature_b64)?;

    // Verify with KMS
    let verify_response = kms_client
        .verify()
        .key_id(kms_key_id)
        .message(Blob::new(hash.to_vec()))
        .signature(Blob::new(signature))
        .signing_algorithm(aws_sdk_kms::types::SigningAlgorithmSpec::RsassaPkcs1V15Sha256)
        .message_type(aws_sdk_kms::types::MessageType::Digest)
        .send()
        .await?;

    if !verify_response.signature_valid() {
        return Err("Invalid JWT signature".into());
    }

    Ok(claims)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    #[test]
    fn test_jwt_claims_serialization() {
        let claims = JwtClaims {
            sub: "user-123".to_string(),
            athlete_id: "i123456".to_string(),
            username: Some("johndoe".to_string()),
            iat: Utc::now().timestamp(),
            exp: (Utc::now() + chrono::Duration::days(7)).timestamp(),
            iss: "https://api.ridelines.xyz".to_string(),
            aud: "ridelines-web".to_string(),
        };

        let json = serde_json::to_string(&claims).unwrap();
        assert!(json.contains("\"sub\":\"user-123\""));
        assert!(json.contains("\"athlete_id\":\"i123456\""));
        assert!(json.contains("\"username\":\"johndoe\""));
    }
}
