use super::{Job, TaskQueue};
use async_trait::async_trait;
use chrono::Utc;
use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
use uuid::Uuid;

pub struct MemoryQueue {
    // Almacenamos bytes para simular el comportamiento de Redis
    queue: Mutex<VecDeque<Vec<u8>>>,
    // Jobs entregados pero aún sin acknowledge (in-flight), por id.
    processing: Mutex<HashMap<String, Vec<u8>>>,
}

impl MemoryQueue {
    pub fn new() -> Self {
        Self {
            queue: Mutex::new(VecDeque::new()),
            processing: Mutex::new(HashMap::new()),
        }
    }
}

impl Default for MemoryQueue {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl TaskQueue for MemoryQueue {
    async fn enqueue(
        &self,
        task_type: &str,
        payload: serde_json::Value,
        trace_id: Option<String>,
    ) -> Result<String, String> {
        let job = Job {
            id: Uuid::new_v4().to_string(),
            task_type: task_type.to_string(),
            payload,
            created_at: Utc::now().timestamp(),
            trace_id: trace_id.unwrap_or_else(|| Uuid::new_v4().to_string()),
        };
        // JSON, no bincode: el payload es `serde_json::Value` y bincode no puede
        // deserializarlo (requiere `deserialize_any`). JSON round-trip seguro.
        let bytes = serde_json::to_vec(&job).map_err(|e| e.to_string())?;
        if let Ok(mut q) = self.queue.lock() {
            q.push_back(bytes);
        }
        Ok(job.id)
    }

    async fn dequeue(&self) -> Result<Option<Job>, String> {
        let bytes = {
            let mut q = self.queue.lock().map_err(|_| "queue lock poisoned")?;
            match q.pop_front() {
                Some(b) => b,
                None => return Ok(None),
            }
        };
        let job: Job = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
        // Mantenemos el job in-flight hasta el acknowledge (semántica at-least-once,
        // coherente con la cola Redis).
        if let Ok(mut p) = self.processing.lock() {
            p.insert(job.id.clone(), bytes);
        }
        Ok(Some(job))
    }

    async fn acknowledge(&self, job_id: &str) -> Result<(), String> {
        if let Ok(mut p) = self.processing.lock() {
            p.remove(job_id);
        }
        Ok(())
    }

    async fn recover_stale(&self) -> Result<usize, String> {
        let items: Vec<Vec<u8>> = {
            let mut p = self
                .processing
                .lock()
                .map_err(|_| "processing lock poisoned")?;
            p.drain().map(|(_, v)| v).collect()
        };
        let count = items.len();
        if count > 0 {
            let mut q = self.queue.lock().map_err(|_| "queue lock poisoned")?;
            for bytes in items {
                q.push_back(bytes);
            }
        }
        Ok(count)
    }
}
