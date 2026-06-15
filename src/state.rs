use crate::auth::authenticator::Authenticator;
use crate::auth::revocation::RevocationStore;
use crate::auth::IdentityProvider;
use crate::core::blackbox::FlightRecorder;
use crate::core::monitor::SystemMonitor;
use crate::core::orchestrator::Orchestrator;
use crate::core::queue::TaskQueue;
use crate::core::ratelimit::RateLimit;
use crate::pulse::PulseReactor;
use crate::store::HybridStore;
use sea_orm::DatabaseConnection;
use std::sync::Arc;

#[derive(Clone)]
pub struct AppState {
    pub db: Arc<DatabaseConnection>,
    pub pulse: Arc<dyn PulseReactor>,
    pub store: HybridStore,
    pub monitor: Arc<SystemMonitor>,
    pub auth: Arc<dyn IdentityProvider>,
    /// Verificador de credenciales (login). BD por defecto; sustituible por
    /// AD/LDAP/otra fuente vía `PulseConfig::authenticator`.
    pub authenticator: Arc<dyn Authenticator>,
    pub blackbox: Arc<dyn FlightRecorder>,
    pub queue: Arc<dyn TaskQueue>,
    pub orchestrator: Option<Arc<Orchestrator>>,
    pub revocations: Arc<dyn RevocationStore>,
    pub rate_limiter: Arc<dyn RateLimit>,
}

impl AppState {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        db: Arc<DatabaseConnection>,
        pulse: Arc<dyn PulseReactor>,
        store: HybridStore,
        auth: Arc<dyn IdentityProvider>,
        authenticator: Arc<dyn Authenticator>,
        blackbox: Arc<dyn FlightRecorder>,
        queue: Arc<dyn TaskQueue>,
        orchestrator: Option<Arc<Orchestrator>>,
        revocations: Arc<dyn RevocationStore>,
        rate_limiter: Arc<dyn RateLimit>,
    ) -> Self {
        Self {
            db,
            pulse,
            store,
            monitor: SystemMonitor::new(),
            auth,
            authenticator,
            blackbox,
            queue,
            orchestrator,
            revocations,
            rate_limiter,
        }
    }
}
