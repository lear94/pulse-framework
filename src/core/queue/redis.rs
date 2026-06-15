use super::{Job, TaskQueue};
use async_trait::async_trait;
use chrono::Utc;
use deadpool_redis::{redis::AsyncCommands, redis::Script, Pool};
use uuid::Uuid;

// Cola fiable estilo "visibility timeout" (semántica SQS / at-least-once):
//   - READY_KEY  (list): ids pendientes. LPUSH a la izquierda, RPOP a la derecha → FIFO.
//   - JOBS_KEY   (hash): id → bytes del Job (bincode). Una sola copia del payload.
//   - INFLIGHT_KEY (zset): id → instante límite del lease (epoch secs). Un job
//     dequeued vive aquí hasta su ACK; si su lease vence sin ACK (worker caído),
//     `recover_stale` lo devuelve a READY. Por nodo NO hay clave separada: el
//     lease por-job evita que un nodo reinyecte jobs que otros nodos procesan.
const READY_KEY: &str = "pulse:queue:ready";
const JOBS_KEY: &str = "pulse:queue:store";
const INFLIGHT_KEY: &str = "pulse:queue:inflight";

/// Ventana de visibilidad: tiempo máximo que un job puede estar in-flight sin
/// ACK antes de considerarse abandonado y reencolarse. Debe superar con holgura
/// la duración del handler más lento; si un handler tarda más, el job podría
/// reentregarse (at-least-once, los handlers deben ser idempotentes).
pub const VISIBILITY_TIMEOUT_SECS: i64 = 300;

/// Periodicidad del reaper (reclamación de leases vencidos). << visibilidad para
/// recuperar pronto el trabajo de un nodo caído sin re-disparar jobs vivos.
pub const REAP_INTERVAL_SECS: u64 = 60;

pub struct RedisQueue {
    pool: Pool,
}

impl RedisQueue {
    pub fn new(pool: Pool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl TaskQueue for RedisQueue {
    async fn enqueue(
        &self,
        task_type: &str,
        payload: serde_json::Value,
        trace_id: Option<String>,
    ) -> Result<String, String> {
        let mut conn = self.pool.get().await.map_err(|e| e.to_string())?;
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
        // Guardamos el payload ANTES de publicar el id: si fallara entre ambos,
        // un id sin payload se descarta limpiamente en dequeue.
        let _: () = conn
            .hset(JOBS_KEY, &job.id, bytes)
            .await
            .map_err(|e| e.to_string())?;
        let _: () = conn
            .lpush(READY_KEY, &job.id)
            .await
            .map_err(|e| e.to_string())?;
        Ok(job.id)
    }

    async fn dequeue(&self) -> Result<Option<Job>, String> {
        let mut conn = self.pool.get().await.map_err(|e| e.to_string())?;
        // Atómico: saca un id de READY, fija su lease en INFLIGHT y devuelve los
        // bytes. Sin ventana de carrera entre pop y registro del lease.
        let lease_deadline = Utc::now().timestamp() + VISIBILITY_TIMEOUT_SECS;
        let script = Script::new(
            r"
            local id = redis.call('RPOP', KEYS[1])
            if not id then return false end
            local data = redis.call('HGET', KEYS[2], id)
            if not data then return false end
            redis.call('ZADD', KEYS[3], ARGV[1], id)
            return data
            ",
        );
        let result: Option<Vec<u8>> = script
            .key(READY_KEY)
            .key(JOBS_KEY)
            .key(INFLIGHT_KEY)
            .arg(lease_deadline)
            .invoke_async(&mut conn)
            .await
            .map_err(|e| e.to_string())?;
        match result {
            Some(bytes) => {
                let job: Job = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
                Ok(Some(job))
            }
            None => Ok(None),
        }
    }

    async fn acknowledge(&self, job_id: &str) -> Result<(), String> {
        let mut conn = self.pool.get().await.map_err(|e| e.to_string())?;
        // O(1): quita el lease y borra el payload. (Antes era O(n) por LRANGE.)
        let script = Script::new(
            r"
            redis.call('ZREM', KEYS[1], ARGV[1])
            redis.call('HDEL', KEYS[2], ARGV[1])
            return 1
            ",
        );
        let _: i64 = script
            .key(INFLIGHT_KEY)
            .key(JOBS_KEY)
            .arg(job_id)
            .invoke_async(&mut conn)
            .await
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    async fn recover_stale(&self) -> Result<usize, String> {
        // Reclama SOLO los leases vencidos (deadline <= ahora): jobs cuyo worker
        // murió entre dequeue y ACK. Los jobs que otros nodos procesan ahora
        // tienen lease futuro y NO se tocan → seguro en multi-nodo.
        let mut conn = self.pool.get().await.map_err(|e| e.to_string())?;
        let now = Utc::now().timestamp();
        let script = Script::new(
            r"
            local ids = redis.call('ZRANGEBYSCORE', KEYS[1], '-inf', ARGV[1])
            for _, id in ipairs(ids) do
                redis.call('ZREM', KEYS[1], id)
                redis.call('LPUSH', KEYS[2], id)
            end
            return #ids
            ",
        );
        let count: usize = script
            .key(INFLIGHT_KEY)
            .key(READY_KEY)
            .arg(now)
            .invoke_async(&mut conn)
            .await
            .map_err(|e| e.to_string())?;
        Ok(count)
    }
}
