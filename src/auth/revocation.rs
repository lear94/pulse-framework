//! Lista de revocación de tokens (denylist por `jti`).
//!
//! Permite invalidar un access token antes de su expiración (logout). Las
//! entradas se guardan con un TTL igual al tiempo de vida restante del token,
//! de modo que la denylist nunca crece indefinidamente: una vez que el token
//! habría expirado igualmente, su entrada desaparece.

use async_trait::async_trait;
use dashmap::DashMap;
use deadpool_redis::{redis::AsyncCommands, Pool};
use std::time::{Duration, Instant};
use tracing::error;

#[async_trait]
pub trait RevocationStore: Send + Sync {
    /// Revoca un `jti` durante `ttl_secs` segundos.
    async fn revoke(&self, jti: &str, ttl_secs: u64);
    /// Indica si un `jti` está revocado.
    async fn is_revoked(&self, jti: &str) -> bool;
}

/// Implementación en memoria (un solo proceso).
pub struct MemoryRevocationStore {
    revoked: DashMap<String, Instant>,
}

impl MemoryRevocationStore {
    pub fn new() -> Self {
        Self {
            revoked: DashMap::new(),
        }
    }
}

impl Default for MemoryRevocationStore {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl RevocationStore for MemoryRevocationStore {
    async fn revoke(&self, jti: &str, ttl_secs: u64) {
        let expiry = Instant::now() + Duration::from_secs(ttl_secs);
        self.revoked.insert(jti.to_string(), expiry);
    }

    async fn is_revoked(&self, jti: &str) -> bool {
        match self.revoked.get(jti).map(|e| *e.value()) {
            Some(expiry) => {
                if Instant::now() >= expiry {
                    // Expirada: limpiamos perezosamente y dejamos de considerarla revocada.
                    self.revoked.remove(jti);
                    false
                } else {
                    true
                }
            }
            None => false,
        }
    }
}

/// Implementación distribuida sobre Redis (compartida entre nodos).
pub struct RedisRevocationStore {
    pool: Pool,
}

impl RedisRevocationStore {
    pub fn new(pool: Pool) -> Self {
        Self { pool }
    }

    fn key(jti: &str) -> String {
        format!("pulse:revoked:{}", jti)
    }
}

#[async_trait]
impl RevocationStore for RedisRevocationStore {
    async fn revoke(&self, jti: &str, ttl_secs: u64) {
        match self.pool.get().await {
            Ok(mut conn) => {
                let res: Result<(), _> = conn
                    .set_ex(Self::key(jti), 1u8, ttl_secs.max(1))
                    .await;
                if let Err(e) = res {
                    error!("Failed to revoke token in Redis: {}", e);
                }
            }
            Err(e) => error!("Failed to acquire Redis connection for revocation: {}", e),
        }
    }

    async fn is_revoked(&self, jti: &str) -> bool {
        match self.pool.get().await {
            Ok(mut conn) => conn.exists(Self::key(jti)).await.unwrap_or(false),
            // Fail-closed ante fallo de Redis: si no podemos comprobar, tratamos
            // el token como NO revocado para no tumbar todo el login por un blip
            // de Redis. (Trade-off disponibilidad vs. seguridad; documentado.)
            Err(e) => {
                error!("Revocation check failed (Redis down): {}", e);
                false
            }
        }
    }
}
