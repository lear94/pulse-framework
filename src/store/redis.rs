use super::{CacheBackend, HybridError, HybridResult};
use async_trait::async_trait;
use dashmap::DashMap;
use deadpool_redis::{
    redis::{AsyncCommands, Client},
    Pool,
};
use futures::StreamExt;
use std::sync::Arc;
use tokio::task;
use tracing::{debug, error, info};
use uuid::Uuid;

const INVALIDATION_CHANNEL: &str = "pulse:cache:invalidate";

/// Mapea cualquier error de driver a `HybridError::Backend` para no filtrar
/// tipos de Redis a través de la abstracción.
fn be<E: std::fmt::Display>(e: E) -> HybridError {
    HybridError::Backend(e.to_string())
}

pub struct RedisBackend {
    pool: Pool,
    url: String,
    // Identidad de esta instancia para distinguir nuestras propias
    // invalidaciones de las de otros nodos.
    instance_id: String,
}

impl RedisBackend {
    pub fn new(pool: Pool, url: String) -> Self {
        Self {
            pool,
            url,
            instance_id: Uuid::new_v4().to_string(),
        }
    }

    async fn listen_loop(
        url: &str,
        instance_id: &str,
        local_cache: Arc<DashMap<String, Vec<u8>>>,
    ) -> HybridResult<()> {
        let client = Client::open(url).map_err(be)?;
        let mut pubsub = client.get_async_pubsub().await.map_err(be)?;
        pubsub.subscribe(INVALIDATION_CHANNEL).await.map_err(be)?;
        let mut stream = pubsub.into_on_message();
        while let Some(msg) = stream.next().await {
            // Formato del mensaje: "<origin_instance_id>|<key>".
            let raw: String = msg.get_payload().map_err(be)?;
            let (origin, key) = match raw.split_once('|') {
                Some((o, k)) => (o, k),
                None => ("", raw.as_str()),
            };
            // Ignoramos nuestras propias invalidaciones: ya escribimos el valor
            // fresco en el caché local, borrarlo provocaría un miss innecesario
            // y rompería el read-your-writes.
            if origin == instance_id {
                continue;
            }
            local_cache.remove(key);
            debug!("Cache INVALIDATED remotely for: {}", key);
        }
        Err(HybridError::NotAvailable)
    }
}

#[async_trait]
impl CacheBackend for RedisBackend {
    async fn get(&self, key: &str) -> HybridResult<Option<Vec<u8>>> {
        let mut conn = self.pool.get().await.map_err(be)?;
        let redis_key = format!("pulse:{}", key);
        let result: Option<Vec<u8>> = conn.get(&redis_key).await.map_err(be)?;
        Ok(result)
    }
    async fn set(&self, key: &str, value: &[u8]) -> HybridResult<()> {
        let mut conn = self.pool.get().await.map_err(be)?;
        let redis_key = format!("pulse:{}", key);
        let _: () = conn.set(&redis_key, value).await.map_err(be)?;
        let _: () = conn
            .publish(INVALIDATION_CHANNEL, format!("{}|{}", self.instance_id, key))
            .await
            .map_err(be)?;
        Ok(())
    }
    async fn del(&self, key: &str) -> HybridResult<()> {
        let mut conn = self.pool.get().await.map_err(be)?;
        let redis_key = format!("pulse:{}", key);
        let _: () = conn.del(&redis_key).await.map_err(be)?;
        let _: () = conn
            .publish(INVALIDATION_CHANNEL, format!("{}|{}", self.instance_id, key))
            .await
            .map_err(be)?;
        Ok(())
    }
    async fn health(&self) -> Option<bool> {
        let mut conn = match self.pool.get().await {
            Ok(c) => c,
            Err(_) => return Some(false),
        };
        let pong: Result<String, _> = deadpool_redis::redis::cmd("PING")
            .query_async(&mut conn)
            .await;
        Some(matches!(pong, Ok(ref p) if p == "PONG"))
    }
    async fn subscribe_to_invalidations(&self, local_cache: Arc<DashMap<String, Vec<u8>>>) {
        let url = self.url.clone();
        let instance_id = self.instance_id.clone();
        task::spawn(async move {
            info!("Redis Invalidation Listener started.");
            loop {
                match Self::listen_loop(&url, &instance_id, local_cache.clone()).await {
                    Ok(_) => debug!("Redis loop closed."),
                    Err(e) => {
                        error!("Redis error: {}. Retry in 5s...", e);
                        tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
                    }
                }
            }
        });
    }
}
