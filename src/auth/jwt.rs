use super::{AuthError, Claims, IdentityProvider};
use async_trait::async_trait;
use chrono::{Duration, Utc};
use jsonwebtoken::{
    decode, encode, errors::ErrorKind, DecodingKey, EncodingKey, Header, Validation,
};
use uuid::Uuid;

pub struct JwtProvider {
    secret: String,
}

impl JwtProvider {
    pub fn new(secret: String) -> Self {
        Self { secret }
    }
}

#[async_trait]
impl IdentityProvider for JwtProvider {
    async fn create_token(&self, user_id: &str, roles: Vec<String>) -> Result<String, AuthError> {
        let now = Utc::now();
        let expiration = now
            .checked_add_signed(Duration::hours(24))
            .ok_or_else(|| AuthError::ProviderError("Clock error".into()))?;

        let claims = Claims {
            sub: user_id.to_owned(),
            iat: now.timestamp() as usize,
            exp: expiration.timestamp() as usize,
            roles,
            trace_id: Uuid::new_v4().to_string(),
        };

        encode(
            &Header::default(),
            &claims,
            &EncodingKey::from_secret(self.secret.as_bytes()),
        )
        .map_err(|e| AuthError::ProviderError(e.to_string()))
    }

    async fn verify_token(&self, token: &str) -> Result<Claims, AuthError> {
        let validation = Validation::default();
        match decode::<Claims>(
            token,
            &DecodingKey::from_secret(self.secret.as_bytes()),
            &validation,
        ) {
            Ok(token_data) => Ok(token_data.claims),
            Err(e) => match e.kind() {
                ErrorKind::ExpiredSignature => Err(AuthError::ExpiredToken),
                ErrorKind::InvalidToken => Err(AuthError::InvalidToken),
                _ => Err(AuthError::InvalidToken),
            },
        }
    }
}
