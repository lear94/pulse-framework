pub mod authenticator;
pub mod jwt;
#[cfg(feature = "ldap")]
pub mod ldap;
pub mod revocation;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
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
