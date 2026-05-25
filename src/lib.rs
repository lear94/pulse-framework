pub mod api;
pub mod auth;
pub mod core;
pub mod models;
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

use crate::auth::jwt::JwtProvider;
use crate::auth::revocation::{MemoryRevocationStore, RedisRevocationStore, RevocationStore};
use crate::auth::IdentityProvider;
use crate::core::blackbox::{
    db::DbRecorder, disk::DiskRecorder, FallbackFlightRecorder, FlightRecorder,
};
use crate::core::orchestrator::Orchestrator;
use crate::core::queue::{memory::MemoryQueue, redis::RedisQueue, TaskQueue};
use crate::core::ratelimit::RateLimiter;
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
}

pub async fn bootstrap<F>(
    config: PulseConfig,
    api_config: F,
    openapi: utoipa::openapi::OpenApi,
) -> std::io::Result<()>
where
    F: FnOnce(&mut web::ServiceConfig) + Send + Copy + 'static,
{
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
        .expect("Failed to connect to DB");
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
        let orch = Arc::new(Orchestrator::new(pool, queue.clone()));
        orch.clone().start().await;
        Some(orch)
    } else {
        warn!("🧠 Orchestrator: Disabled (Local Mode).");
        None
    };

    // El secret JWT NO tiene default: arrancar con un valor conocido permitiría
    // forjar tokens. Exigimos uno explícito y razonablemente fuerte.
    let jwt_secret = std::env::var("JWT_SECRET")
        .expect("JWT_SECRET must be set (refusing to start with an insecure default)");
    if jwt_secret.len() < 16 {
        panic!("JWT_SECRET is too weak: provide at least 16 characters");
    }
    let access_ttl: i64 = std::env::var("PULSE_ACCESS_TTL_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(3600);
    let refresh_ttl: i64 = std::env::var("PULSE_REFRESH_TTL_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(7 * 24 * 3600);
    let auth_provider: Arc<dyn IdentityProvider> =
        Arc::new(JwtProvider::with_ttls(jwt_secret, access_ttl, refresh_ttl));

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
    let rate_limiter = Arc::new(RateLimiter::new(rl_max, Duration::from_secs(rl_window)));

    let db_recorder = Arc::new(DbRecorder::new(db.clone()));
    let disk_recorder = Arc::new(DiskRecorder::new());
    let blackbox: Arc<dyn FlightRecorder> =
        Arc::new(FallbackFlightRecorder::new(db_recorder, disk_recorder));

    let state = AppState::new(
        db,
        pulse_reactor,
        store,
        auth_provider,
        blackbox,
        queue.clone(),
        orchestrator,
        revocations,
        rate_limiter,
        redis_pool.clone(),
    );

    // Recuperación de jobs in-flight de una ejecución anterior (worker caído
    // entre dequeue y acknowledge) antes de empezar a consumir.
    match queue.recover_stale().await {
        Ok(0) => {}
        Ok(n) => warn!("♻️ Recovered {} in-flight job(s) from a previous run.", n),
        Err(e) => warn!("Failed to recover stale jobs: {}", e),
    }

    // Canal de apagado: al recibir SIGINT/SIGTERM dejamos de tomar jobs nuevos
    // pero terminamos (y ack) el que esté en curso, evitando perder trabajo.
    let (shutdown_tx, mut worker_shutdown) = tokio::sync::watch::channel(false);

    let queue_clone = queue.clone();
    let worker = tokio::spawn(async move {
        loop {
            if *worker_shutdown.borrow() {
                break;
            }
            match queue_clone.dequeue().await {
                Ok(Some(job)) => {
                    info!("⚙️ Processing Job: {} [{}]", job.id, job.task_type);
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    let _ = queue_clone.acknowledge(&job.id).await;
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

    let server = HttpServer::new(move || {
        let mut cors = Cors::default()
            .allow_any_method()
            .allow_any_header()
            .max_age(3600);
        for origin in &cors_origins {
            cors = cors.allowed_origin(origin.as_str());
        }

        App::new()
            .wrap(middleware::Compress::default())
            .wrap(TracingLogger::default())
            .wrap(cors)
            .wrap(middleware::DefaultHeaders::new().add(("X-Pulse-Version", "3.0-Autonomy")))
            .app_data(web::Data::new(state.clone()))
            .configure(api_config)
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
