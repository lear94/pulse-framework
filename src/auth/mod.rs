pub mod jwt;

use crate::state::AppState;
use actix_web::{dev::Payload, error::ErrorUnauthorized, web, FromRequest, HttpRequest};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::future::Future;
use std::pin::Pin;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum AuthError {
    #[error("Invalid token")]
    InvalidToken,
    #[error("Token expired")]
    ExpiredToken,
    #[error("Missing credentials")]
    MissingCredentials,
    #[error("Insufficient permissions")]
    Forbidden,
    #[error("Internal provider error: {0}")]
    ProviderError(String),
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Claims {
    pub sub: String,
    pub exp: usize,
    pub iat: usize,
    #[serde(default)]
    pub roles: Vec<String>,
    #[serde(default)]
    pub trace_id: String,
}

impl Claims {
    pub fn has_role(&self, role: &str) -> bool {
        self.roles.iter().any(|r| r == role)
    }
}

#[async_trait]
pub trait IdentityProvider: Send + Sync {
    async fn create_token(&self, user_id: &str, roles: Vec<String>) -> Result<String, AuthError>;
    async fn verify_token(&self, token: &str) -> Result<Claims, AuthError>;
}

impl FromRequest for Claims {
    type Error = actix_web::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self, Self::Error>>>>;

    fn from_request(req: &HttpRequest, _payload: &mut Payload) -> Self::Future {
        let req = req.clone();
        Box::pin(async move {
            let state = req
                .app_data::<web::Data<AppState>>()
                .ok_or_else(|| ErrorUnauthorized("App state not found"))?;

            let auth_header = req.headers().get("Authorization");

            match auth_header {
                Some(auth_val) => {
                    let auth_str = auth_val.to_str().unwrap_or("");
                    if !auth_str.starts_with("Bearer ") {
                        return Err(ErrorUnauthorized("Invalid token format"));
                    }
                    let token = &auth_str[7..];

                    state
                        .auth
                        .verify_token(token)
                        .await
                        .map_err(|_| ErrorUnauthorized("Invalid or expired token"))
                }
                None => Err(ErrorUnauthorized("Missing Authorization header")),
            }
        })
    }
}
