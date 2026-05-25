pub mod memory;
pub mod redis;

use async_trait::async_trait;
use dashmap::DashMap;
use deadpool_redis::{redis::RedisError, PoolError};
use serde::{de::DeserializeOwned, Serialize};
use std::sync::Arc;
use tokio::task;
use tracing::{debug, error};

pub type HybridResult<T> = Result<T, HybridError>;

#[derive(thiserror::Error, Debug)]
pub enum HybridError {
    #[error("Backend connection error: {0}")]
    Pool(#[from] PoolError),
    #[error("Backend command error: {0}")]
    Redis(#[from] RedisError),
    #[error("Serialization error: {0}")]
    Serde(#[from] bincode::Error),
    #[error("Backend is not available")]
    NotAvailable,
}

const LOCAL_CACHE_CAPACITY: usize = 10_000;

#[async_trait]
pub trait CacheBackend: Send + Sync {
    async fn get(&self, key: &str) -> HybridResult<Option<Vec<u8>>>;
    async fn set(&self, key: &str, value: &[u8]) -> HybridResult<()>;
    async fn del(&self, key: &str) -> HybridResult<()>;
    async fn subscribe_to_invalidations(&self, local_cache: Arc<DashMap<String, Vec<u8>>>);
}

#[derive(Clone)]
pub struct HybridStore {
    local_data: Arc<DashMap<String, Vec<u8>>>,
    backend: Arc<dyn CacheBackend>,
}

impl HybridStore {
    pub fn new(backend: Arc<dyn CacheBackend>) -> Self {
        let store = Self {
            local_data: Arc::new(DashMap::new()),
            backend: backend.clone(),
        };
        let local_ref = store.local_data.clone();
        let backend_ref = store.backend.clone();
        task::spawn(async move {
            backend_ref.subscribe_to_invalidations(local_ref).await;
        });
        store
    }

    fn enforce_capacity(&self) {
        // Acota el tamaño aunque varias tareas inserten en paralelo: evicta en
        // bucle hasta dejar hueco para una entrada nueva. (Eviction aleatoria,
        // no LRU: DashMap no preserva orden de acceso.)
        while self.local_data.len() >= LOCAL_CACHE_CAPACITY {
            let victim = self.local_data.iter().next().map(|r| r.key().clone());
            match victim {
                Some(key) => {
                    self.local_data.remove(&key);
                }
                None => break,
            }
        }
    }

    pub async fn set<T: Serialize>(&self, key: &str, value: &T) -> HybridResult<()> {
        let bytes = bincode::serialize(value)?;
        self.enforce_capacity();
        self.local_data.insert(key.to_string(), bytes.clone());
        self.backend.set(key, &bytes).await?;
        debug!("Cache SET propagation complete: {}", key);
        Ok(())
    }

    pub async fn get<T: DeserializeOwned>(&self, key: &str) -> Option<T> {
        if let Some(r) = self.local_data.get(key) {
            debug!("Cache HIT (Layer 1): {}", key);
            return bincode::deserialize(&r).ok();
        }
        match self.backend.get(key).await {
            Ok(Some(bytes)) => {
                self.enforce_capacity();
                self.local_data.insert(key.to_string(), bytes.clone());
                debug!("Cache HIT (Layer 2) & HYDRATED: {}", key);
                bincode::deserialize(&bytes).ok()
            }
            Ok(None) => {
                debug!("Cache MISS (Layers 1 & 2): {}", key);
                None
            }
            Err(e) => {
                error!("Cache Backend GET FAILED: {}", e);
                None
            }
        }
    }

    pub async fn del(&self, key: &str) -> HybridResult<()> {
        self.local_data.remove(key);
        self.backend.del(key).await?;
        Ok(())
    }

    pub fn local_count(&self) -> usize {
        self.local_data.len()
    }
}
