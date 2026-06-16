use crate::auth::authenticator::Authenticator;
use crate::auth::revocation::RevocationStore;
use crate::auth::IdentityProvider;
use crate::core::blackbox::FlightRecorder;
use crate::core::monitor::SystemMonitor;
use crate::core::orchestrator::Orchestrator;
use crate::core::queue::TaskQueue;
use crate::core::ratelimit::RateLimit;
use crate::persistence::{Datastore, UserRepository};
use crate::pulse::PulseReactor;
use crate::store::HybridStore;
use std::sync::Arc;

#[derive(Clone)]
pub struct AppState {
    /// Persistencia del dominio tras un trait: el ORM concreto es intercambiable.
    pub users: Arc<dyn UserRepository>,
    /// Health-check del almacén (aísla el pool concreto del ORM).
    pub datastore: Arc<dyn Datastore>,
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
        users: Arc<dyn UserRepository>,
        datastore: Arc<dyn Datastore>,
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
            users,
            datastore,
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
