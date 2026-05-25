//! Rate limiter en memoria, ventana fija, sin dependencias externas.
//!
//! Es un limitador *por proceso*: en un despliegue multi-nodo cada instancia
//! cuenta por separado. Para límites globales se necesitaría un backend
//! compartido (p.ej. Redis con INCR + EXPIRE); este cubre el caso de un nodo y
//! mitiga abuso/fuerza bruta básica.

use dashmap::DashMap;
use std::time::{Duration, Instant};

pub struct RateLimiter {
    hits: DashMap<String, (u32, Instant)>,
    max: u32,
    window: Duration,
    // Cota dura de claves rastreadas para que un atacante rotando IPs no
    // haga crecer el mapa sin límite.
    max_keys: usize,
}

impl RateLimiter {
    pub fn new(max_requests: u32, window: Duration) -> Self {
        Self {
            hits: DashMap::new(),
            max: max_requests,
            window,
            max_keys: 100_000,
        }
    }

    /// Devuelve `true` si la petición está permitida y la contabiliza.
    /// `key` suele ser "ruta:ip".
    pub fn check(&self, key: &str) -> bool {
        let now = Instant::now();

        // Protección contra crecimiento ilimitado: si el mapa está saturado,
        // purgamos entradas cuya ventana ya expiró.
        if self.hits.len() >= self.max_keys {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_after_max() {
        let rl = RateLimiter::new(3, Duration::from_secs(60));
        assert!(rl.check("login:1.2.3.4"));
        assert!(rl.check("login:1.2.3.4"));
        assert!(rl.check("login:1.2.3.4"));
        assert!(!rl.check("login:1.2.3.4")); // 4ª excede
        assert!(rl.check("login:5.6.7.8")); // otra IP no afectada
    }
}
