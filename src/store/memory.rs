use super::{CacheBackend, HybridResult};
use async_trait::async_trait;
use dashmap::DashMap;
use std::sync::Arc;

pub struct MemoryBackend;

#[async_trait]
impl CacheBackend for MemoryBackend {
    async fn get(&self, _key: &str) -> HybridResult<Option<Vec<u8>>> {
        Ok(None)
    }
    async fn set(&self, _key: &str, _value: &[u8]) -> HybridResult<()> {
        Ok(())
    }
    async fn del(&self, _key: &str) -> HybridResult<()> {
        Ok(())
    }
    async fn subscribe_to_invalidations(&self, _local_cache: Arc<DashMap<String, Vec<u8>>>) {}
    async fn health(&self) -> Option<bool> {
        None // Sin dependencia externa: modo local.
    }
}
