pub mod memory;
pub mod redis;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PulseSignal {
    UserCreated(String),
}

#[async_trait]
pub trait PulseReactor: Send + Sync {
    async fn emit(&self, signal: PulseSignal);
    fn subscribe(&self) -> broadcast::Receiver<PulseSignal>;
}
