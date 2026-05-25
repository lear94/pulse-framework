use crate::auth::revocation::RevocationStore;
use crate::auth::IdentityProvider;
use crate::core::blackbox::FlightRecorder;
use crate::core::monitor::SystemMonitor;
use crate::core::orchestrator::Orchestrator;
use crate::core::queue::TaskQueue;
use crate::core::ratelimit::RateLimiter;
use crate::pulse::PulseReactor;
use crate::store::HybridStore;
use deadpool_redis::Pool;
use sea_orm::DatabaseConnection;
use std::sync::Arc;

#[derive(Clone)]
pub struct AppState {
    pub db: Arc<DatabaseConnection>,
    pub pulse: Arc<dyn PulseReactor>,
    pub store: HybridStore,
    pub monitor: Arc<SystemMonitor>,
    pub auth: Arc<dyn IdentityProvider>,
    pub blackbox: Arc<dyn FlightRecorder>,
    pub queue: Arc<dyn TaskQueue>,
    pub orchestrator: Option<Arc<Orchestrator>>,
    pub revocations: Arc<dyn RevocationStore>,
    pub rate_limiter: Arc<RateLimiter>,
    /// Pool de Redis para chequeos de salud (None en modo local).
    pub redis_pool: Option<Pool>,
}

impl AppState {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        db: Arc<DatabaseConnection>,
        pulse: Arc<dyn PulseReactor>,
        store: HybridStore,
        auth: Arc<dyn IdentityProvider>,
        blackbox: Arc<dyn FlightRecorder>,
        queue: Arc<dyn TaskQueue>,
        orchestrator: Option<Arc<Orchestrator>>,
        revocations: Arc<dyn RevocationStore>,
        rate_limiter: Arc<RateLimiter>,
        redis_pool: Option<Pool>,
    ) -> Self {
        Self {
            db,
            pulse,
            store,
            monitor: SystemMonitor::new(),
            auth,
            blackbox,
            queue,
            orchestrator,
            revocations,
            rate_limiter,
            redis_pool,
        }
    }
}
