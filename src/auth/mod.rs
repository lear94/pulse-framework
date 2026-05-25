pub mod jwt;
pub mod revocation;

use crate::state::AppState;
use actix_web::{
    dev::Payload,
    error::{ErrorForbidden, ErrorUnauthorized},
    web, FromRequest, HttpRequest,
};
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
    /// Identificador único del token (para revocación / logout).
    #[serde(default)]
    pub jti: String,
    /// Tipo de token: "access" o "refresh".
    #[serde(default = "default_kind")]
    pub kind: String,
    #[serde(default)]
    pub roles: Vec<String>,
    #[serde(default)]
    pub trace_id: String,
}

fn default_kind() -> String {
    "access".to_string()
}

impl Claims {
    pub fn has_role(&self, role: &str) -> bool {
        self.roles.iter().any(|r| r == role)
    }
    pub fn is_access(&self) -> bool {
        self.kind == "access"
    }
    pub fn is_refresh(&self) -> bool {
        self.kind == "refresh"
    }
    /// Segundos que faltan para que el token expire (0 si ya expiró).
    pub fn remaining_ttl_secs(&self) -> u64 {
        let now = chrono::Utc::now().timestamp();
        (self.exp as i64 - now).max(0) as u64
    }
}

#[async_trait]
pub trait IdentityProvider: Send + Sync {
    /// Access token de vida corta.
    async fn create_token(&self, user_id: &str, roles: Vec<String>) -> Result<String, AuthError>;
    /// Refresh token de vida larga (kind = "refresh").
    async fn create_refresh_token(
        &self,
        user_id: &str,
        roles: Vec<String>,
    ) -> Result<String, AuthError>;
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

                    let claims = state
                        .auth
                        .verify_token(token)
                        .await
                        .map_err(|_| ErrorUnauthorized("Invalid or expired token"))?;

                    // Solo los access tokens autorizan endpoints; un refresh
                    // token no debe usarse como credencial de API.
                    if !claims.is_access() {
                        return Err(ErrorUnauthorized(
                            "Refresh tokens cannot be used to access APIs",
                        ));
                    }
                    // Denylist: tokens revocados (logout) dejan de valer.
                    if !claims.jti.is_empty() && state.revocations.is_revoked(&claims.jti).await {
                        return Err(ErrorUnauthorized("Token has been revoked"));
                    }
                    Ok(claims)
                }
                None => Err(ErrorUnauthorized("Missing Authorization header")),
            }
        })
    }
}

/// Extractor que exige un JWT válido **con el rol `admin`**.
/// Las rutas administrativas deben usar este extractor en lugar de `Claims`.
pub struct AdminClaims(pub Claims);

impl FromRequest for AdminClaims {
    type Error = actix_web::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self, Self::Error>>>>;

    fn from_request(req: &HttpRequest, payload: &mut Payload) -> Self::Future {
        let claims_fut = Claims::from_request(req, payload);
        Box::pin(async move {
            let claims = claims_fut.await?;
            if claims.has_role("admin") {
                Ok(AdminClaims(claims))
            } else {
                Err(ErrorForbidden("Admin role required"))
            }
        })
    }
}
