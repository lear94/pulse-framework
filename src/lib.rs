// Motor (transport-agnostic): cero dependencia de Actix.
pub mod auth;
pub mod core;
pub mod persistence;
pub mod pulse;
pub mod services;
pub mod state;
pub mod store;

// Cuarentena HTTP: TODO Actix vive bajo `web` (adaptador `api` + `bootstrap`).
pub mod web;

// Compatibilidad de paths públicos (cli/apps generadas usan estos).
pub use web::api;
pub use web::{bootstrap, PulseConfig};

pub use actix_web;
pub use async_trait;
pub use chrono;
pub use dotenvy;
pub use serde;
pub use serde_json;
pub use tokio;
pub use tracing;
pub use utoipa;
pub use utoipa_swagger_ui;
pub use uuid;
