use super::{FlightRecord, FlightRecorder};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sea_orm::entity::prelude::*;
use sea_orm::{
    ActiveModelBehavior, ActiveValue::Set, DatabaseConnection, DeriveEntityModel, DeriveRelation,
    EntityTrait, EnumIter, PrimaryKeyTrait, QueryOrder, QuerySelect,
};
use serde_json::Value;
use std::sync::Arc;
use uuid::Uuid;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "blackbox_records")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub handler: String,
    pub payload: Value,
    pub error: String,
    pub timestamp: DateTime<Utc>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}

pub struct DbRecorder {
    db: Arc<DatabaseConnection>,
}

impl DbRecorder {
    pub fn new(db: Arc<DatabaseConnection>) -> Self {
        Self { db }
    }
}

#[async_trait]
impl FlightRecorder for DbRecorder {
    async fn record(&self, record: FlightRecord) -> Result<(), String> {
        let active_model = ActiveModel {
            id: Set(Uuid::parse_str(&record.id).map_err(|e| e.to_string())?),
            handler: Set(record.handler),
            payload: Set(record.payload),
            error: Set(record.error),
            timestamp: Set(DateTime::parse_from_rfc3339(&record.timestamp)
                .map_err(|e| e.to_string())?
                .with_timezone(&Utc)),
        };
        active_model
            .insert(self.db.as_ref())
            .await
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    async fn tail(&self, limit: usize) -> Vec<FlightRecord> {
        match Entity::find()
            .order_by_desc(Column::Timestamp)
            .limit(limit as u64)
            .all(self.db.as_ref())
            .await
        {
            Ok(models) => models
                .into_iter()
                .map(|m| FlightRecord {
                    id: m.id.to_string(),
                    handler: m.handler,
                    payload: m.payload,
                    error: m.error,
                    timestamp: m.timestamp.to_rfc3339(),
                })
                .collect(),
            Err(_) => Vec::new(),
        }
    }

    async fn scan_id(&self, id: &str) -> Option<FlightRecord> {
        let uuid = Uuid::parse_str(id).ok()?;
        let model = Entity::find_by_id(uuid)
            .one(self.db.as_ref())
            .await
            .ok()??;
        Some(FlightRecord {
            id: model.id.to_string(),
            handler: model.handler,
            payload: model.payload,
            error: model.error,
            timestamp: model.timestamp.to_rfc3339(),
        })
    }
}
