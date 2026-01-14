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

const INVALIDATION_CHANNEL: &str = "pulse:cache:invalidate";

pub struct RedisBackend {
    pool: Pool,
    url: String,
}

impl RedisBackend {
    pub fn new(pool: Pool, url: String) -> Self {
        Self { pool, url }
    }

    async fn listen_loop(
        url: &str,
        local_cache: Arc<DashMap<String, Vec<u8>>>,
    ) -> HybridResult<()> {
        let client = Client::open(url).map_err(HybridError::Redis)?;
        let mut pubsub = client
            .get_async_pubsub()
            .await
            .map_err(HybridError::Redis)?;
        pubsub
            .subscribe(INVALIDATION_CHANNEL)
            .await
            .map_err(HybridError::Redis)?;
        let mut stream = pubsub.into_on_message();
        while let Some(msg) = stream.next().await {
            let key_to_invalidate: String = msg.get_payload().map_err(HybridError::Redis)?;
            local_cache.remove(&key_to_invalidate);
            debug!("Cache INVALIDATED remotely for: {}", key_to_invalidate);
        }
        Err(HybridError::NotAvailable)
    }
}

#[async_trait]
impl CacheBackend for RedisBackend {
    async fn get(&self, key: &str) -> HybridResult<Option<Vec<u8>>> {
        let mut conn = self.pool.get().await?;
        let redis_key = format!("pulse:{}", key);
        let result: Option<Vec<u8>> = conn.get(&redis_key).await?;
        Ok(result)
    }
    async fn set(&self, key: &str, value: &[u8]) -> HybridResult<()> {
        let mut conn = self.pool.get().await?;
        let redis_key = format!("pulse:{}", key);
        let _: () = conn.set(&redis_key, value).await?;
        let _: () = conn.publish(INVALIDATION_CHANNEL, key).await?;
        Ok(())
    }
    async fn del(&self, key: &str) -> HybridResult<()> {
        let mut conn = self.pool.get().await?;
        let redis_key = format!("pulse:{}", key);
        let _: () = conn.del(&redis_key).await?;
        let _: () = conn.publish(INVALIDATION_CHANNEL, key).await?;
        Ok(())
    }
    async fn subscribe_to_invalidations(&self, local_cache: Arc<DashMap<String, Vec<u8>>>) {
        let url = self.url.clone();
        task::spawn(async move {
            info!("Redis Invalidation Listener started.");
            loop {
                match Self::listen_loop(&url, local_cache.clone()).await {
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
