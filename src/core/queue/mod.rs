pub mod memory;
pub mod redis;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Job {
    pub id: String,
    pub task_type: String,
    pub payload: serde_json::Value,
    pub created_at: i64,
    pub trace_id: String,
}

#[async_trait]
pub trait TaskQueue: Send + Sync {
    async fn enqueue(
        &self,
        task_type: &str,
        payload: serde_json::Value,
        trace_id: Option<String>,
    ) -> Result<String, String>;
    async fn dequeue(&self) -> Result<Option<Job>, String>;
    async fn acknowledge(&self, job_id: &str) -> Result<(), String>;
}
