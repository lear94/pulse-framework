use super::{Job, TaskQueue};
use async_trait::async_trait;
use chrono::Utc;
use std::sync::Mutex;
use std::collections::VecDeque;
use uuid::Uuid;

pub struct MemoryQueue {
    // Almacenamos bytes para simular el comportamiento de Redis
    queue: Mutex<VecDeque<Vec<u8>>>,
}

impl MemoryQueue {
    pub fn new() -> Self {
        Self { queue: Mutex::new(VecDeque::new()) }
    }
}

#[async_trait]
impl TaskQueue for MemoryQueue {
    async fn enqueue(&self, task_type: &str, payload: serde_json::Value, trace_id: Option<String>) -> Result<String, String> {
        let job = Job {
            id: Uuid::new_v4().to_string(),
            task_type: task_type.to_string(),
            payload,
            created_at: Utc::now().timestamp(),
            trace_id: trace_id.unwrap_or_else(|| Uuid::new_v4().to_string()),
        };
        let bytes = bincode::serialize(&job).map_err(|e| e.to_string())?;
        if let Ok(mut q) = self.queue.lock() { q.push_back(bytes); }
        Ok(job.id)
    }

    async fn dequeue(&self) -> Result<Option<Job>, String> {
        if let Ok(mut q) = self.queue.lock() {
            if let Some(bytes) = q.pop_front() {
                let job: Job = bincode::deserialize(&bytes).map_err(|e| e.to_string())?;
                return Ok(Some(job));
            }
        }
        Ok(None)
    }

    async fn acknowledge(&self, _job_id: &str) -> Result<(), String> { Ok(()) }
}
