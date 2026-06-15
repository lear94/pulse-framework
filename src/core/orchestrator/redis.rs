//! Implementación Redis del [`Coordinator`]: lock de líder vía `SET NX` + ZSET
//! de tareas programadas. Es el único punto del scheduler que conoce Redis.

use super::{Coordinator, ScheduledTask};
use async_trait::async_trait;
use deadpool_redis::{redis::AsyncCommands, Pool};
use tracing::warn;
use uuid::Uuid;

const LEADER_KEY: &str = "pulse:cluster:leader";
const SCHEDULE_KEY: &str = "pulse:cluster:schedule";

/// TTL del lock de líder. Renovado cada ciclo (3:1) y tras cada tarea para que
/// un ciclo largo no pierda el liderato.
const LEADER_TTL_SECS: i64 = 15;

pub struct RedisCoordinator {
    pool: Pool,
    node_id: String,
}

impl RedisCoordinator {
    pub fn new(pool: Pool) -> Self {
        Self {
            pool,
            node_id: Uuid::new_v4().to_string(),
        }
    }
}

#[async_trait]
impl Coordinator for RedisCoordinator {
    async fn try_acquire_leadership(&self) -> Result<bool, String> {
        let mut conn = self.pool.get().await.map_err(|e| e.to_string())?;
        let opts = deadpool_redis::redis::SetOptions::default()
            .conditional_set(deadpool_redis::redis::ExistenceCheck::NX)
            .with_expiration(deadpool_redis::redis::SetExpiry::EX(LEADER_TTL_SECS as u64));

        let acquired: Option<String> = conn
            .set_options(LEADER_KEY, &self.node_id, opts)
            .await
            .map_err(|e| e.to_string())?;

        match acquired {
            Some(_) => Ok(true),
            None => {
                // El lock pudo expirar entre el SET NX y el GET: tratamos la
                // ausencia como "no somos líder" en vez de fallar.
                let current: Option<String> =
                    conn.get(LEADER_KEY).await.map_err(|e| e.to_string())?;
                if current.as_deref() == Some(self.node_id.as_str()) {
                    let _: () = conn
                        .expire(LEADER_KEY, LEADER_TTL_SECS)
                        .await
                        .map_err(|e| e.to_string())?;
                    Ok(true)
                } else {
                    Ok(false)
                }
            }
        }
    }

    async fn renew_leadership(&self) -> Result<(), String> {
        let mut conn = self.pool.get().await.map_err(|e| e.to_string())?;
        // Fencing: solo renovamos si el lock sigue siendo nuestro.
        let current: Option<String> = conn.get(LEADER_KEY).await.map_err(|e| e.to_string())?;
        if current.as_deref() == Some(self.node_id.as_str()) {
            let _: () = conn
                .expire(LEADER_KEY, LEADER_TTL_SECS)
                .await
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    async fn schedule_at(
        &self,
        task_type: &str,
        payload: serde_json::Value,
        timestamp: i64,
    ) -> Result<(), String> {
        let mut conn = self.pool.get().await.map_err(|e| e.to_string())?;
        // JSON en el ZSET por inspeccionabilidad (debug). Al encolar, TaskQueue
        // convierte a Bincode.
        let job_data = serde_json::json!({
            "type": task_type,
            "payload": payload,
            "trace_id": Uuid::new_v4().to_string()
        });
        let _: () = conn
            .zadd(SCHEDULE_KEY, job_data.to_string(), timestamp as f64)
            .await
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    async fn due_tasks(&self, now: i64, batch: usize) -> Result<Vec<ScheduledTask>, String> {
        let mut conn = self.pool.get().await.map_err(|e| e.to_string())?;
        let raw_tasks: Vec<String> = conn
            .zrangebyscore_limit(SCHEDULE_KEY, "-inf", now as f64, 0, batch as isize)
            .await
            .map_err(|e| e.to_string())?;

        let mut out = Vec::with_capacity(raw_tasks.len());
        for raw in raw_tasks {
            match serde_json::from_str::<serde_json::Value>(&raw) {
                Ok(value) => out.push(ScheduledTask {
                    task_type: value["type"].as_str().unwrap_or("unknown").to_string(),
                    payload: value["payload"].clone(),
                    trace_id: value["trace_id"].as_str().map(|s| s.to_string()),
                    handle: raw,
                }),
                Err(_) => {
                    // JSON corrupto: lo removemos para no atascar el scheduler.
                    warn!("scheduler: dropping corrupt schedule entry");
                    let _: usize = conn
                        .zrem(SCHEDULE_KEY, &raw)
                        .await
                        .map_err(|e| e.to_string())?;
                }
            }
        }
        Ok(out)
    }

    async fn ack_scheduled(&self, task: &ScheduledTask) -> Result<(), String> {
        let mut conn = self.pool.get().await.map_err(|e| e.to_string())?;
        let _: usize = conn
            .zrem(SCHEDULE_KEY, &task.handle)
            .await
            .map_err(|e| e.to_string())?;
        Ok(())
    }
}
