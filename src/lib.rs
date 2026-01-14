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
use crate::auth::IdentityProvider;
use crate::core::blackbox::{
    db::DbRecorder, disk::DiskRecorder, FallbackFlightRecorder, FlightRecorder,
};
use crate::core::orchestrator::Orchestrator;
use crate::core::queue::{memory::MemoryQueue, redis::RedisQueue, TaskQueue};
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

    let jwt_secret = std::env::var("JWT_SECRET").unwrap_or_else(|_| "secret".into());
    let auth_provider: Arc<dyn IdentityProvider> = Arc::new(JwtProvider::new(jwt_secret));

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
    );

    let queue_clone = queue.clone();
    tokio::spawn(async move {
        loop {
            if let Ok(Some(job)) = queue_clone.dequeue().await {
                info!("⚙️ Processing Job: {} [{}]", job.id, job.task_type);
                tokio::time::sleep(Duration::from_millis(50)).await;
                let _ = queue_clone.acknowledge(&job.id).await;
            } else {
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        }
    });

    tokio::spawn(async move {
        while let Ok(signal) = rx_pulse.recv().await {
            tracing::info!(target: "pulse_global", "⚡ SIGNAL: {:?}", signal);
        }
    });

    tracing::info!("🚀 Pulse Engine Ignited at {}:{}", config.host, config.port);

    HttpServer::new(move || {
        App::new()
            .wrap(middleware::Compress::default())
            .wrap(TracingLogger::default())
            .wrap(Cors::permissive())
            .wrap(middleware::DefaultHeaders::new().add(("X-Pulse-Version", "3.0-Autonomy")))
            .app_data(web::Data::new(state.clone()))
            .configure(api_config)
            .service(
                SwaggerUi::new("/swagger-ui/{_:.*}").url("/api-docs/openapi.json", openapi.clone()),
            )
    })
    .bind((config.host.as_str(), config.port))?
    .run()
    .await
}
