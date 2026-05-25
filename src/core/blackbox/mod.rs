pub mod db;
pub mod disk;

use async_trait::async_trait;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::{error, info, warn};
use utoipa::ToSchema;

pub use db::DbRecorder;
pub use disk::DiskRecorder;

#[derive(Serialize, Deserialize, Debug, ToSchema, Clone)]
pub struct FlightRecord {
    pub id: String,
    pub handler: String,
    pub payload: serde_json::Value,
    pub error: String,
    #[schema(value_type = String, format = "date-time")]
    pub timestamp: String,
}

impl FlightRecord {
    pub fn new(handler: &str, payload: serde_json::Value, error: String) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            handler: handler.to_string(),
            payload,
            error,
            timestamp: Utc::now().to_rfc3339(),
        }
    }
}

#[async_trait]
pub trait FlightRecorder: Send + Sync {
    async fn record(&self, record: FlightRecord) -> Result<(), String>;
    async fn tail(&self, limit: usize) -> Vec<FlightRecord>;
    async fn scan_id(&self, id: &str) -> Option<FlightRecord>;
}

pub struct FallbackFlightRecorder {
    primary: Arc<dyn FlightRecorder>,
    backup: Arc<dyn FlightRecorder>,
}

impl FallbackFlightRecorder {
    pub fn new(primary: Arc<dyn FlightRecorder>, backup: Arc<dyn FlightRecorder>) -> Self {
        Self { primary, backup }
    }
}

#[async_trait]
impl FlightRecorder for FallbackFlightRecorder {
    async fn record(&self, record: FlightRecord) -> Result<(), String> {
        match self.primary.record(record.clone()).await {
            Ok(_) => {
                info!("FlightRecord saved to Primary Storage.");
                Ok(())
            }
            Err(e) => {
                error!("Primary Storage Failed: {}. Engaging Backup Systems.", e);
                match self.backup.record(record).await {
                    Ok(_) => {
                        warn!("FlightRecord saved to Backup Storage (Degraded Mode).");
                        Ok(())
                    }
                    Err(e_backup) => {
                        error!(
                            "CRITICAL: All Blackbox systems failed. Record lost: {}",
                            e_backup
                        );
                        Err(format!("Primary: {}, Backup: {}", e, e_backup))
                    }
                }
            }
        }
    }

    async fn tail(&self, limit: usize) -> Vec<FlightRecord> {
        // Combinamos primary y backup: tras una caída del primary, parte de los
        // registros pueden vivir solo en el backup. Si nos quedáramos con el
        // primary cuando no está vacío, esos registros quedarían invisibles.
        let mut combined = self.primary.tail(limit).await;
        let mut seen: std::collections::HashSet<String> =
            combined.iter().map(|r| r.id.clone()).collect();
        for record in self.backup.tail(limit).await {
            if seen.insert(record.id.clone()) {
                combined.push(record);
            }
        }
        // timestamp es RFC3339 en UTC para ambos backends → orden lexicográfico
        // equivale a orden cronológico. Más reciente primero.
        combined.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
        combined.truncate(limit);
        combined
    }

    async fn scan_id(&self, id: &str) -> Option<FlightRecord> {
        if let Some(record) = self.primary.scan_id(id).await {
            return Some(record);
        }
        self.backup.scan_id(id).await
    }
}
