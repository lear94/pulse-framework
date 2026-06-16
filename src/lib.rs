pub mod api;
pub mod auth;
pub mod core;
pub mod models;
pub mod persistence;
pub mod pulse;
pub mod services;
pub mod state;
pub mod store;

pub use actix_web;
pub use async_trait;
pub use chrono;
pub use dotenvy;
pub use sea_orm;
pub use serde;
pub use serde_json;
pub use tokio;
pub use tracing;
pub use utoipa;
pub use utoipa_swagger_ui;
pub use uuid;

use crate::auth::authenticator::{Authenticator, DbAuthenticator};
use crate::auth::jwt::JwtProvider;
use crate::auth::revocation::{MemoryRevocationStore, RedisRevocationStore, RevocationStore};
use crate::auth::IdentityProvider;
use crate::core::blackbox::{
    db::DbRecorder, disk::DiskRecorder, FallbackFlightRecorder, FlightRecorder,
};
use crate::core::orchestrator::{redis::RedisCoordinator, Orchestrator};
use crate::core::queue::{
    memory::MemoryQueue,
    redis::{RedisQueue, REAP_INTERVAL_SECS},
    JobHandlers, TaskQueue,
};
use crate::core::ratelimit::{MemoryRateLimiter, RateLimit, RedisRateLimiter};
use crate::persistence::seaorm::{SeaOrmDatastore, SeaOrmUserRepository};
use crate::persistence::{Datastore, UserRepository};
use crate::services::recovery_service::RecoveryService;
use crate::pulse::{memory::MemoryReactor, redis::RedisReactor, PulseReactor};
use crate::state::AppState;
use crate::store::{memory::MemoryBackend, redis::RedisBackend, CacheBackend, HybridStore};
use actix_cors::Cors;
use actix_web::{middleware, web, App, HttpServer};
use deadpool_redis::{Config as RedisConfig, Runtime as RedisRuntime};
use sea_orm::{ConnectOptions, Database};
use std::sync::Arc;
use std::time::Duration;
use tracing::{info, warn};
use tracing_actix_web::TracingLogger;
use utoipa_swagger_ui::SwaggerUi;

pub struct PulseConfig {
    pub database_url: String,
    pub redis_url: Option<String>,
    pub host: String,
    pub port: u16,
    pub db_max_connections: u32,
    /// Handlers de jobs indexados por `task_type`. El worker despacha cada job
    /// dequeued a su handler; un `task_type` sin handler se descarta (con warn)
    /// para no quedar reintentándolo indefinidamente.
    pub handlers: JobHandlers,
    /// Verificación de credenciales (el "método de login"). `None` → BD por
    /// defecto ([`DbAuthenticator`]). Inyecta aquí AD/LDAP/otra tabla.
    pub authenticator: Option<Arc<dyn Authenticator>>,
    /// Emisión/validación de tokens. `None` → JWT (`JwtProvider`) configurado
    /// desde el entorno (`JWT_SECRET`, TTLs). Inyecta aquí para tokens propios.
    pub auth_provider: Option<Arc<dyn IdentityProvider>>,
}

impl Default for PulseConfig {
    fn default() -> Self {
        Self {
            database_url: String::new(),
            redis_url: None,
            host: "127.0.0.1".to_string(),
            port: 8080,
            db_max_connections: 10,
            handlers: JobHandlers::default(),
            authenticator: None,
            auth_provider: None,
        }
    }
}

pub async fn bootstrap<F>(
    config: PulseConfig,
    api_config: F,
    openapi: utoipa::openapi::OpenApi,
) -> std::io::Result<()>
where
    F: Fn(&mut web::ServiceConfig) + Send + Clone + 'static,
{
    use std::io::{Error, ErrorKind};
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_target(false)
        .compact()
        .init();

    let mut opt = ConnectOptions::new(&config.database_url);
    opt.max_connections(config.db_max_connections)
        .min_connections(5)
        .connect_timeout(Duration::from_secs(8))
        .sqlx_logging(false);

    let db_conn = Database::connect(opt)
        .await
        .map_err(|e| Error::other(format!("Failed to connect to DB: {e}")))?;
    let db = Arc::new(db_conn);

    let redis_pool = match config.redis_url.clone() {
        Some(url) => {
            let redis_cfg = RedisConfig::from_url(&url);
            redis_cfg.create_pool(Some(RedisRuntime::Tokio1)).ok()
        }
        None => None,
    };

    let cache_backend: Arc<dyn CacheBackend> = match (redis_pool.clone(), config.redis_url.clone())
    {
        (Some(pool), Some(url)) => Arc::new(RedisBackend::new(pool, url)),
        _ => Arc::new(MemoryBackend),
    };
    let store = HybridStore::new(cache_backend);

    let (pulse_reactor, mut rx_pulse): (Arc<dyn PulseReactor>, _) =
        match (redis_pool.clone(), config.redis_url.clone()) {
            (Some(pool), Some(url)) => {
                let (reactor, rx) = RedisReactor::new(pool, url);
                (reactor, rx)
            }
            _ => {
                let (reactor, rx) = MemoryReactor::new(500);
                (reactor, rx)
            }
        };

    let queue: Arc<dyn TaskQueue> = match redis_pool.clone() {
        Some(pool) => Arc::new(RedisQueue::new(pool)),
        None => Arc::new(MemoryQueue::new()),
    };

    let orchestrator = if let Some(pool) = redis_pool.clone() {
        info!("🧠 Orchestrator: Initializing Distributed Brain.");
        let coordinator = Arc::new(RedisCoordinator::new(pool));
        let orch = Arc::new(Orchestrator::new(coordinator, queue.clone()));
        orch.clone().start().await;
        Some(orch)
    } else {
        warn!("🧠 Orchestrator: Disabled (Local Mode).");
        None
    };

    // IdentityProvider: si la app inyecta el suyo, lo usamos tal cual; si no,
    // construimos el JWT por defecto desde el entorno. El secret JWT NO tiene
    // default (arrancar con uno conocido permitiría forjar tokens): se exige
    // explícito y razonablemente fuerte SOLO cuando usamos el JWT por defecto.
    let auth_provider: Arc<dyn IdentityProvider> = match config.auth_provider.clone() {
        Some(custom) => custom,
        None => {
            let jwt_secret = std::env::var("JWT_SECRET").map_err(|_| {
                Error::new(
                    ErrorKind::InvalidInput,
                    "JWT_SECRET must be set (refusing to start with an insecure default)",
                )
            })?;
            const MIN_JWT_SECRET_LEN: usize = 16;
            if jwt_secret.len() < MIN_JWT_SECRET_LEN {
                return Err(Error::new(
                    ErrorKind::InvalidInput,
                    "JWT_SECRET is too weak: provide at least 16 characters",
                ));
            }
            let access_ttl: i64 = std::env::var("PULSE_ACCESS_TTL_SECS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(3600);
            let refresh_ttl: i64 = std::env::var("PULSE_REFRESH_TTL_SECS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(7 * 24 * 3600);
            Arc::new(JwtProvider::with_ttls(jwt_secret, access_ttl, refresh_ttl))
        }
    };

    // Authenticator: el "método de login". BD por defecto; AD/LDAP/otra tabla
    // si la app lo inyecta.
    let authenticator: Arc<dyn Authenticator> = config
        .authenticator
        .clone()
        .unwrap_or_else(|| Arc::new(DbAuthenticator));

    // Denylist de tokens (logout): distribuida si hay Redis, en memoria si no.
    let revocations: Arc<dyn RevocationStore> = match redis_pool.clone() {
        Some(pool) => Arc::new(RedisRevocationStore::new(pool)),
        None => Arc::new(MemoryRevocationStore::new()),
    };

    // Rate limiter (por IP, por proceso). Configurable por entorno.
    let rl_max: u32 = std::env::var("PULSE_RATE_LIMIT_MAX")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(10);
    let rl_window: u64 = std::env::var("PULSE_RATE_LIMIT_WINDOW_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(60);
    // Con Redis el límite es GLOBAL (compartido entre nodos); sin él, por proceso.
    let rl_window_dur = Duration::from_secs(rl_window);
    let rate_limiter: Arc<dyn RateLimit> = match redis_pool.clone() {
        Some(pool) => Arc::new(RedisRateLimiter::new(pool, rl_max, rl_window_dur)),
        None => Arc::new(MemoryRateLimiter::new(rl_max, rl_window_dur)),
    };

    let db_recorder = Arc::new(DbRecorder::new(db.clone()));
    let disk_recorder = Arc::new(DiskRecorder::new());
    let blackbox: Arc<dyn FlightRecorder> =
        Arc::new(FallbackFlightRecorder::new(db_recorder, disk_recorder));

    // Repositorios de dominio tras traits: aquí (bootstrap) es el ÚNICO sitio
    // que conoce el ORM concreto. Cambiar de ORM = cambiar estas dos líneas.
    let users: Arc<dyn UserRepository> = Arc::new(SeaOrmUserRepository::new(db.clone()));
    let datastore: Arc<dyn Datastore> = Arc::new(SeaOrmDatastore::new(db.clone()));

    let state = AppState::new(
        users,
        datastore,
        pulse_reactor,
        store,
        auth_provider,
        authenticator,
        blackbox,
        queue.clone(),
        orchestrator,
        revocations,
        rate_limiter,
    );

    // Recuperación de jobs in-flight de una ejecución anterior (worker caído
    // entre dequeue y acknowledge) antes de empezar a consumir.
    match queue.recover_stale().await {
        Ok(0) => {}
        Ok(n) => warn!("♻️ Recovered {} in-flight job(s) from a previous run.", n),
        Err(e) => warn!("Failed to recover stale jobs: {}", e),
    }

    // Reaper: solo con Redis. Reclama periódicamente los leases vencidos (jobs de
    // nodos caídos). En modo memoria NO se ejecuta: `recover_stale` allí vacía
    // TODO el in-flight y reentregaría el job que este mismo proceso procesa.
    if redis_pool.is_some() {
        let reaper_queue = queue.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(REAP_INTERVAL_SECS));
            loop {
                tick.tick().await;
                match reaper_queue.recover_stale().await {
                    Ok(0) => {}
                    Ok(n) => warn!("♻️ Reaper re-queued {} stale job(s).", n),
                    Err(e) => warn!("Reaper failed: {}", e),
                }
            }
        });
    }

    // Canal de apagado: al recibir SIGINT/SIGTERM dejamos de tomar jobs nuevos
    // pero terminamos (y ack) el que esté en curso, evitando perder trabajo.
    let (shutdown_tx, mut worker_shutdown) = tokio::sync::watch::channel(false);

    let queue_clone = queue.clone();
    let worker_handlers = config.handlers;
    let worker_state = state.clone();
    let worker = tokio::spawn(async move {
        loop {
            if *worker_shutdown.borrow() {
                break;
            }
            match queue_clone.dequeue().await {
                Ok(Some(job)) => {
                    info!("⚙️ Processing Job: {} [{}]", job.id, job.task_type);
                    match worker_handlers.get(&job.task_type) {
                        Some(handler) => match handler.handle(&job).await {
                            Ok(()) => {
                                let _ = queue_clone.acknowledge(&job.id).await;
                            }
                            Err(e) => {
                                warn!("Job {} [{}] failed: {}", job.id, job.task_type, e);
                                RecoveryService::capture_failure(
                                    &worker_state,
                                    &job.task_type,
                                    job.payload.clone(),
                                    e,
                                )
                                .await;
                                // Ack para no reintentar un job venenoso en bucle;
                                // queda en el blackbox para replay manual.
                                let _ = queue_clone.acknowledge(&job.id).await;
                            }
                        },
                        None => {
                            warn!(
                                "No handler registered for task_type '{}'; dropping job {}",
                                job.task_type, job.id
                            );
                            let _ = queue_clone.acknowledge(&job.id).await;
                        }
                    }
                }
                _ => {
                    // Espera 1s o despierta de inmediato si llega el apagado.
                    tokio::select! {
                        _ = tokio::time::sleep(Duration::from_secs(1)) => {}
                        _ = worker_shutdown.changed() => {}
                    }
                }
            }
        }
        info!("🧵 Job worker stopped gracefully.");
    });

    tokio::spawn(async move {
        while let Ok(signal) = rx_pulse.recv().await {
            tracing::info!(target: "pulse_global", "⚡ SIGNAL: {:?}", signal);
        }
    });

    tracing::info!("🚀 Pulse Engine Ignited at {}:{}", config.host, config.port);

    // CORS configurable por entorno. Sin `PULSE_CORS_ORIGINS` el comportamiento
    // por defecto es restrictivo (no permitir orígenes cruzados), en lugar del
    // antiguo `Cors::permissive()` que aceptaba cualquier origen.
    let cors_origins: Vec<String> = std::env::var("PULSE_CORS_ORIGINS")
        .map(|s| {
            s.split(',')
                .map(|o| o.trim().to_string())
                .filter(|o| !o.is_empty())
                .collect()
        })
        .unwrap_or_default();
    if cors_origins.is_empty() {
        warn!("CORS: no PULSE_CORS_ORIGINS set — cross-origin requests will be denied.");
    }

    // Clonamos el monitor fuera del factory (que mueve `state`) para contabilizar
    // TODA petición HTTP, no solo un handler concreto.
    let monitor = state.monitor.clone();
    let server = HttpServer::new(move || {
        let mut cors = Cors::default()
            .allow_any_method()
            .allow_any_header()
            .max_age(3600);
        for origin in &cors_origins {
            cors = cors.allowed_origin(origin.as_str());
        }

        let req_monitor = monitor.clone();
        App::new()
            .wrap_fn(move |req, srv| {
                use actix_web::dev::Service as _;
                use std::sync::atomic::Ordering::Relaxed;
                let m = req_monitor.clone();
                m.requests_total.fetch_add(1, Relaxed);
                m.active_connections.fetch_add(1, Relaxed);
                let fut = srv.call(req);
                async move {
                    let res = fut.await;
                    m.active_connections.fetch_sub(1, Relaxed);
                    res
                }
            })
            .wrap(middleware::Compress::default())
            .wrap(TracingLogger::default())
            .wrap(cors)
            .wrap(middleware::DefaultHeaders::new().add(("X-Pulse-Version", "3.0-Autonomy")))
            .app_data(web::Data::new(state.clone()))
            .configure(api_config.clone())
            .service(
                SwaggerUi::new("/swagger-ui/{_:.*}").url("/api-docs/openapi.json", openapi.clone()),
            )
    })
    .bind((config.host.as_str(), config.port))?
    // Da tiempo a que las conexiones en curso terminen antes de cerrar.
    .shutdown_timeout(30)
    .run();

    // actix gestiona SIGINT/SIGTERM y termina `server` limpiamente.
    let server_result = server.await;

    // Una vez parado el HTTP server, paramos el worker y esperamos a que el job
    // en curso (si lo hay) termine y haga ack.
    tracing::info!("🛑 Shutdown signal received: draining job worker...");
    let _ = shutdown_tx.send(true);
    if tokio::time::timeout(Duration::from_secs(30), worker)
        .await
        .is_err()
    {
        warn!("Job worker did not stop within 30s; forcing exit.");
    }

    server_result
}
