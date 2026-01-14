use super::{Job, TaskQueue};
use async_trait::async_trait;
use chrono::Utc;
use deadpool_redis::{redis::AsyncCommands, Pool};
use uuid::Uuid;

const QUEUE_KEY: &str = "pulse:queue:jobs";
const PROCESSING_KEY: &str = "pulse:queue:processing";

pub struct RedisQueue {
    pool: Pool,
}

impl RedisQueue {
    pub fn new(pool: Pool) -> Self { Self { pool } }
}

#[async_trait]
impl TaskQueue for RedisQueue {
    async fn enqueue(&self, task_type: &str, payload: serde_json::Value, trace_id: Option<String>) -> Result<String, String> {
        let mut conn = self.pool.get().await.map_err(|e| e.to_string())?;
        let job = Job {
            id: Uuid::new_v4().to_string(),
            task_type: task_type.to_string(),
            payload,
            created_at: Utc::now().timestamp(),
            trace_id: trace_id.unwrap_or_else(|| Uuid::new_v4().to_string()),
        };
        // [OPT] Serialización binaria para Redis
        let bytes = bincode::serialize(&job).map_err(|e| e.to_string())?;
        let _: () = conn.rpush(QUEUE_KEY, bytes).await.map_err(|e| e.to_string())?;
        Ok(job.id)
    }

    async fn dequeue(&self) -> Result<Option<Job>, String> {
        let mut conn = self.pool.get().await.map_err(|e| e.to_string())?;
        // Redis devuelve bytes
        let result: Option<Vec<u8>> = conn.rpoplpush(QUEUE_KEY, PROCESSING_KEY).await.map_err(|e| e.to_string())?;
        match result {
            Some(bytes) => {
                let job: Job = bincode::deserialize(&bytes).map_err(|e| e.to_string())?;
                Ok(Some(job))
            },
            None => Ok(None)
        }
    }

    async fn acknowledge(&self, job_id: &str) -> Result<(), String> {
        let mut conn = self.pool.get().await.map_err(|e| e.to_string())?;
        // Para ack, necesitamos iterar. Esto es costoso en binario sin índices, pero aceptable para colas medianas.
        // En un sistema V7, usaríamos ZSETs para esto.
        // Por ahora mantenemos la lógica pero deserializamos.
        let pending: Vec<Vec<u8>> = conn.lrange(PROCESSING_KEY, 0, -1).await.map_err(|e| e.to_string())?;
        for bytes in pending {
            if let Ok(job) = bincode::deserialize::<Job>(&bytes) {
                if job.id == job_id {
                    let _: () = conn.lrem(PROCESSING_KEY, 1, bytes).await.map_err(|e| e.to_string())?;
                    return Ok(());
                }
            }
        }
        Ok(())
    }
}
