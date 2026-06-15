//! Scheduler distribuido agnóstico al backend.
//!
//! El `Orchestrator` aporta la lógica genérica (campaña de liderato + drenaje de
//! tareas vencidas a la cola); las primitivas distribuidas (lock de líder y store
//! de tareas programadas) viven tras el trait [`Coordinator`]. Hoy la única impl
//! es [`redis::RedisCoordinator`], pero el core ya no menciona Redis: otro backend
//! (etcd, Postgres advisory locks, NATS…) solo necesita implementar `Coordinator`.

pub mod redis;

use crate::core::queue::TaskQueue;
use async_trait::async_trait;
use chrono::Utc;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::sleep;
use tracing::info;

/// Periodicidad de campaña/renovación del liderato.
const LEADER_RENEW_SECS: u64 = 5;
/// Tareas movidas por ciclo (acota la latencia de un ciclo de `process_schedule`).
const SCHEDULE_BATCH: usize = 10;

/// Tarea programada lista para encolarse. `handle` es un token opaco que el
/// backend usa para confirmar su despacho (eliminarla del store).
pub struct ScheduledTask {
    pub task_type: String,
    pub payload: serde_json::Value,
    pub trace_id: Option<String>,
    pub handle: String,
}

/// Primitivas distribuidas que requiere el scheduler. Backend-específicas.
#[async_trait]
pub trait Coordinator: Send + Sync {
    /// Intenta adquirir (o confirmar) el liderato. `true` si somos líder.
    async fn try_acquire_leadership(&self) -> Result<bool, String>;
    /// Renueva el TTL del liderato SOLO si seguimos siendo líder (fencing).
    async fn renew_leadership(&self) -> Result<(), String>;
    /// Programa una tarea para ejecutarse a partir de `timestamp` (epoch secs).
    async fn schedule_at(
        &self,
        task_type: &str,
        payload: serde_json::Value,
        timestamp: i64,
    ) -> Result<(), String>;
    /// Tareas con vencimiento <= `now`, hasta `batch`. El backend descarta y
    /// limpia entradas corruptas internamente.
    async fn due_tasks(&self, now: i64, batch: usize) -> Result<Vec<ScheduledTask>, String>;
    /// Confirma que `task` fue despachada (la elimina del store).
    async fn ack_scheduled(&self, task: &ScheduledTask) -> Result<(), String>;
}

pub struct Orchestrator {
    coordinator: Arc<dyn Coordinator>,
    queue: Arc<dyn TaskQueue>,
}

impl Orchestrator {
    pub fn new(coordinator: Arc<dyn Coordinator>, queue: Arc<dyn TaskQueue>) -> Self {
        Self { coordinator, queue }
    }

    pub async fn start(self: Arc<Self>) {
        let orchestrator = self.clone();
        tokio::spawn(async move {
            loop {
                orchestrator.tick().await;
                sleep(Duration::from_secs(LEADER_RENEW_SECS)).await;
            }
        });
    }

    /// Una iteración del scheduler: solo el líder drena tareas. Aislada de los
    /// sleeps para poder ejercitarla determinísticamente en tests.
    async fn tick(&self) {
        if let Ok(true) = self.coordinator.try_acquire_leadership().await {
            let _ = self.process_schedule().await;
        }
    }

    /// API pública para que las apps programen tareas diferidas.
    pub async fn schedule_at(
        &self,
        task_type: &str,
        payload: serde_json::Value,
        timestamp: i64,
    ) -> Result<(), String> {
        self.coordinator.schedule_at(task_type, payload, timestamp).await
    }

    async fn process_schedule(&self) -> Result<(), String> {
        let now = Utc::now().timestamp();
        let tasks = self.coordinator.due_tasks(now, SCHEDULE_BATCH).await?;
        if !tasks.is_empty() {
            info!("👑 Leader: moving {} scheduled task(s) to the queue.", tasks.len());
        }
        for task in tasks {
            // Encolamos PRIMERO y solo entonces confirmamos (at-least-once): si
            // encolar falla, la tarea permanece y se reintenta el próximo ciclo.
            self.queue
                .enqueue(&task.task_type, task.payload.clone(), task.trace_id.clone())
                .await?;
            self.coordinator.ack_scheduled(&task).await?;
            // Renovamos el lease tras cada tarea: un batch lento no pierde el liderato.
            self.coordinator.renew_leadership().await?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::queue::memory::MemoryQueue;
    use crate::core::queue::Job;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    /// Coordinator en memoria: control total de liderato y tareas vencidas, y
    /// recuento de ack/renovaciones para verificar el protocolo del Orchestrator.
    struct MockCoordinator {
        is_leader: bool,
        due: Mutex<Vec<ScheduledTask>>,
        acked: Mutex<Vec<String>>,
        renews: AtomicUsize,
        acquire_calls: AtomicUsize,
    }

    impl MockCoordinator {
        fn new(is_leader: bool, due: Vec<ScheduledTask>) -> Self {
            Self {
                is_leader,
                due: Mutex::new(due),
                acked: Mutex::new(Vec::new()),
                renews: AtomicUsize::new(0),
                acquire_calls: AtomicUsize::new(0),
            }
        }
        fn task(handle: &str) -> ScheduledTask {
            ScheduledTask {
                task_type: "email".into(),
                payload: serde_json::json!({ "to": handle }),
                trace_id: None,
                handle: handle.into(),
            }
        }
    }

    #[async_trait]
    impl Coordinator for MockCoordinator {
        async fn try_acquire_leadership(&self) -> Result<bool, String> {
            self.acquire_calls.fetch_add(1, Ordering::SeqCst);
            Ok(self.is_leader)
        }
        async fn renew_leadership(&self) -> Result<(), String> {
            self.renews.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
        async fn schedule_at(&self, t: &str, _p: serde_json::Value, _ts: i64) -> Result<(), String> {
            self.due.lock().map_err(|_| "poisoned")?.push(Self::task(t));
            Ok(())
        }
        async fn due_tasks(&self, _now: i64, batch: usize) -> Result<Vec<ScheduledTask>, String> {
            let mut due = self.due.lock().map_err(|_| "poisoned")?;
            let n = batch.min(due.len());
            Ok(due.drain(..n).collect())
        }
        async fn ack_scheduled(&self, task: &ScheduledTask) -> Result<(), String> {
            self.acked
                .lock()
                .map_err(|_| "poisoned")?
                .push(task.handle.clone());
            Ok(())
        }
    }

    /// Cola que siempre falla al encolar: para verificar que SIN encolar no se
    /// hace ack (la tarea sobrevive en el store → at-least-once).
    struct FailingQueue;
    #[async_trait]
    impl TaskQueue for FailingQueue {
        async fn enqueue(
            &self,
            _t: &str,
            _p: serde_json::Value,
            _tr: Option<String>,
        ) -> Result<String, String> {
            Err("enqueue boom".into())
        }
        async fn dequeue(&self) -> Result<Option<Job>, String> {
            Ok(None)
        }
        async fn acknowledge(&self, _id: &str) -> Result<(), String> {
            Ok(())
        }
        async fn recover_stale(&self) -> Result<usize, String> {
            Ok(0)
        }
    }

    async fn drain_count(q: &MemoryQueue) -> usize {
        let mut n = 0;
        while let Ok(Some(_)) = q.dequeue().await {
            n += 1;
        }
        n
    }

    /// Bloquea ignorando el envenenamiento (sin `.unwrap()`): en un test un panic
    /// previo ya falla la prueba; aquí solo nos interesa el contenido.
    fn locked<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
        m.lock().unwrap_or_else(|e| e.into_inner())
    }

    #[tokio::test]
    async fn leader_drains_all_due_tasks_then_acks_and_renews() {
        let coord = Arc::new(MockCoordinator::new(
            true,
            vec![
                MockCoordinator::task("a"),
                MockCoordinator::task("b"),
                MockCoordinator::task("c"),
            ],
        ));
        let queue = Arc::new(MemoryQueue::new());
        let orch = Orchestrator::new(coord.clone(), queue.clone());

        orch.tick().await;

        assert_eq!(drain_count(&queue).await, 3, "las 3 tareas deben encolarse");
        assert_eq!(locked(&coord.acked).len(), 3, "cada tarea se confirma");
        assert_eq!(coord.renews.load(Ordering::SeqCst), 3, "renovación por tarea");
        assert!(locked(&coord.due).is_empty(), "store vaciado");
    }

    #[tokio::test]
    async fn non_leader_does_nothing() {
        let coord = Arc::new(MockCoordinator::new(false, vec![MockCoordinator::task("a")]));
        let queue = Arc::new(MemoryQueue::new());
        let orch = Orchestrator::new(coord.clone(), queue.clone());

        orch.tick().await;

        assert_eq!(drain_count(&queue).await, 0, "un no-líder no encola nada");
        assert!(locked(&coord.acked).is_empty());
        assert_eq!(locked(&coord.due).len(), 1, "tareas intactas");
        assert_eq!(coord.acquire_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn enqueue_failure_does_not_ack() {
        // Si encolar falla, NO debe confirmarse la tarea: en un backend real
        // permanecería en el store para reintentarse (at-least-once).
        let coord = Arc::new(MockCoordinator::new(true, vec![MockCoordinator::task("a")]));
        let orch = Orchestrator::new(coord.clone(), Arc::new(FailingQueue));

        orch.tick().await; // tick traga el error; el invariante es el no-ack

        assert!(
            locked(&coord.acked).is_empty(),
            "sin encolar no hay ack"
        );
    }

    #[tokio::test]
    async fn batch_is_capped() {
        let due: Vec<_> = (0..(SCHEDULE_BATCH + 5))
            .map(|i| MockCoordinator::task(&i.to_string()))
            .collect();
        let coord = Arc::new(MockCoordinator::new(true, due));
        let queue = Arc::new(MemoryQueue::new());
        let orch = Orchestrator::new(coord.clone(), queue.clone());

        orch.tick().await;

        assert_eq!(
            drain_count(&queue).await,
            SCHEDULE_BATCH,
            "un ciclo no excede el batch"
        );
        assert_eq!(locked(&coord.due).len(), 5, "el resto queda pendiente");
    }
}
