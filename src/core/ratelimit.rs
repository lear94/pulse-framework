//! Rate limiting con ventana fija, tras el trait `RateLimit`.
//!
//! Dos backends:
//!   - `MemoryRateLimiter`: por proceso, sin dependencias. En multi-nodo cada
//!     instancia cuenta por separado, así que el límite efectivo se multiplica
//!     por el número de réplicas. Cubre un solo nodo y mitiga fuerza bruta.
//!   - `RedisRateLimiter`: límite GLOBAL compartido entre nodos (INCR+EXPIRE).
//!     Fail-open ante caída de Redis (documentado): preferimos seguir sirviendo
//!     a bloquear todo el login por un blip de Redis.

use async_trait::async_trait;
use dashmap::DashMap;
use deadpool_redis::{redis::Script, Pool};
use std::time::{Duration, Instant};
use tracing::warn;

#[async_trait]
pub trait RateLimit: Send + Sync {
    /// `true` si la petición está permitida y la contabiliza. `key` ~ "ruta:ip".
    async fn check(&self, key: &str) -> bool;
}

/// Cota dura de claves rastreadas en memoria para que un atacante rotando IPs
/// no haga crecer el mapa sin límite.
const MAX_TRACKED_KEYS: usize = 100_000;

pub struct MemoryRateLimiter {
    hits: DashMap<String, (u32, Instant)>,
    max: u32,
    window: Duration,
}

impl MemoryRateLimiter {
    pub fn new(max_requests: u32, window: Duration) -> Self {
        Self {
            hits: DashMap::new(),
            max: max_requests,
            window,
        }
    }

    fn check_sync(&self, key: &str) -> bool {
        let now = Instant::now();

        // Si el mapa está saturado, purgamos entradas cuya ventana ya expiró.
        if self.hits.len() >= MAX_TRACKED_KEYS {
            self.hits
                .retain(|_, (_, start)| now.duration_since(*start) <= self.window);
        }

        let mut entry = self.hits.entry(key.to_string()).or_insert((0, now));
        if now.duration_since(entry.1) > self.window {
            *entry = (0, now);
        }
        if entry.0 >= self.max {
            return false;
        }
        entry.0 += 1;
        true
    }
}

#[async_trait]
impl RateLimit for MemoryRateLimiter {
    async fn check(&self, key: &str) -> bool {
        self.check_sync(key)
    }
}

/// Límite distribuido (compartido entre nodos) sobre Redis: `INCR` del contador
/// de la ventana y `EXPIRE` en el primer hit para que se reinicie sola.
pub struct RedisRateLimiter {
    pool: Pool,
    max: u32,
    window_secs: u64,
}

impl RedisRateLimiter {
    pub fn new(pool: Pool, max_requests: u32, window: Duration) -> Self {
        Self {
            pool,
            max: max_requests,
            window_secs: window.as_secs().max(1),
        }
    }

    fn key(k: &str) -> String {
        format!("pulse:rl:{}", k)
    }
}

#[async_trait]
impl RateLimit for RedisRateLimiter {
    async fn check(&self, key: &str) -> bool {
        let mut conn = match self.pool.get().await {
            Ok(c) => c,
            // Fail-open: sin Redis no podemos contar; servir es preferible a
            // bloquear todo el tráfico.
            Err(e) => {
                warn!("rate limiter: Redis unavailable ({e}); allowing (fail-open)");
                return true;
            }
        };
        let rkey = Self::key(key);
        // INCR + EXPIRE atómico: el EXPIRE en el primer hit no puede perderse
        // (antes, fallar entre ambos dejaba la clave sin TTL → lockout permanente).
        let script = Script::new(
            r"
            local c = redis.call('INCR', KEYS[1])
            if c == 1 then redis.call('EXPIRE', KEYS[1], ARGV[1]) end
            return c
            ",
        );
        let count: i64 = match script
            .key(&rkey)
            .arg(self.window_secs as i64)
            .invoke_async(&mut conn)
            .await
        {
            Ok(n) => n,
            Err(e) => {
                warn!("rate limiter script failed ({e}); allowing (fail-open)");
                return true;
            }
        };
        count <= self.max as i64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_after_max() {
        let rl = MemoryRateLimiter::new(3, Duration::from_secs(60));
        assert!(rl.check_sync("login:1.2.3.4"));
        assert!(rl.check_sync("login:1.2.3.4"));
        assert!(rl.check_sync("login:1.2.3.4"));
        assert!(!rl.check_sync("login:1.2.3.4")); // 4ª excede
        assert!(rl.check_sync("login:5.6.7.8")); // otra IP no afectada
    }
}
