use crate::core::queue::TaskQueue;
use chrono::Utc;
use deadpool_redis::{redis::AsyncCommands, Pool};
use std::sync::Arc;
use std::time::Duration;
use tokio::time::sleep;
use tracing::info;
use uuid::Uuid;

const LEADER_KEY: &str = "pulse:cluster:leader";
const SCHEDULE_KEY: &str = "pulse:cluster:schedule";

pub struct Orchestrator {
    pool: Pool,
    node_id: String,
    queue: Arc<dyn TaskQueue>,
}

impl Orchestrator {
    pub fn new(pool: Pool, queue: Arc<dyn TaskQueue>) -> Self {
        Self {
            pool,
            node_id: Uuid::new_v4().to_string(),
            queue,
        }
    }

    pub async fn start(self: Arc<Self>) {
        let orchestrator = self.clone();
        tokio::spawn(async move {
            loop {
                if let Ok(is_leader) = orchestrator.campaign().await {
                    if is_leader {
                        let _ = orchestrator.process_schedule().await;
                    }
                }
                sleep(Duration::from_secs(5)).await;
            }
        });
    }

    async fn campaign(&self) -> Result<bool, String> {
        let mut conn = self.pool.get().await.map_err(|e| e.to_string())?;
        let opts = deadpool_redis::redis::SetOptions::default()
            .conditional_set(deadpool_redis::redis::ExistenceCheck::NX)
            .with_expiration(deadpool_redis::redis::SetExpiry::EX(10));

        let result: Option<String> = conn.set_options(LEADER_KEY, &self.node_id, opts).await.map_err(|e| e.to_string())?;

        match result {
            Some(_) => {
                let _: () = conn.expire(LEADER_KEY, 10).await.map_err(|e| e.to_string())?;
                Ok(true)
            },
            None => {
                let current_leader: String = conn.get(LEADER_KEY).await.map_err(|e| e.to_string())?;
                if current_leader == self.node_id {
                    let _: () = conn.expire(LEADER_KEY, 10).await.map_err(|e| e.to_string())?;
                    Ok(true)
                } else {
                    Ok(false)
                }
            }
        }
    }

    pub async fn schedule_at(&self, task_type: &str, payload: serde_json::Value, timestamp: i64) -> Result<(), String> {
        let mut conn = self.pool.get().await.map_err(|e| e.to_string())?;
        // El scheduler sigue usando JSON para inspección, o podría usar Bincode.
        // Por compatibilidad con herramientas de debug, lo dejamos JSON en el ZSET.
        let job_data = serde_json::json!({
            "type": task_type,
            "payload": payload,
            "trace_id": Uuid::new_v4().to_string()
        });
        let json = job_data.to_string();
        let _: () = conn.zadd(SCHEDULE_KEY, json, timestamp as f64).await.map_err(|e| e.to_string())?;
        Ok(())
    }

    async fn process_schedule(&self) -> Result<(), String> {
        let mut conn = self.pool.get().await.map_err(|e| e.to_string())?;
        let now = Utc::now().timestamp() as f64;
        let tasks: Vec<String> = conn.zrangebyscore_limit(SCHEDULE_KEY, "-inf", now, 0, 10).await.map_err(|e| e.to_string())?;

        if !tasks.is_empty() {
            info!("👑 Leader [Node {}]: Moving {} scheduled tasks to Queue.", &self.node_id[..8], tasks.len());
        }

        for task_json in tasks {
            let removed: usize = conn.zrem(SCHEDULE_KEY, &task_json).await.map_err(|e| e.to_string())?;
            if removed > 0 {
                if let Ok(value) = serde_json::from_str::<serde_json::Value>(&task_json) {
                    let t_type = value["type"].as_str().unwrap_or("unknown");
                    let payload = value["payload"].clone();
                    let trace = value["trace_id"].as_str().map(|s| s.to_string());
                    // Al encolar, TaskQueue convierte a Bincode automáticamente
                    self.queue.enqueue(t_type, payload, trace).await?;
                }
            }
        }
        Ok(())
    }
}
