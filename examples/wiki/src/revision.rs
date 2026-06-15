//! Entidad `page_revisions` — historial de versiones de cada página.
//!
//! Cada guardado (creación o edición) añade una fila inmutable con el contenido
//! de esa versión. El número de revisión es secuencial por página; la constraint
//! UNIQUE(page_id, revision) — definida en el esquema — es el guardián ante
//! carreras al calcular el siguiente número.

use async_trait::async_trait;
use sea_orm::entity::prelude::*;
use sea_orm::ActiveValue::{NotSet, Set};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize, ToSchema)]
#[sea_orm(table_name = "page_revisions")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    #[serde(default)]
    pub id: Uuid,
    pub page_id: Uuid,
    /// Número de revisión, 1-based y secuencial por página.
    pub revision: i32,
    pub title: String,
    pub content: String,
    /// Quién hizo este cambio (puede diferir del autor original de la página).
    pub author: String,
    #[serde(skip_deserializing)]
    pub created_at: DateTime,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

#[async_trait]
impl ActiveModelBehavior for ActiveModel {
    async fn before_save<C>(self, _db: &C, insert: bool) -> Result<Self, DbErr>
    where
        C: ConnectionTrait,
    {
        let mut model = self;
        if insert {
            if let NotSet = model.id {
                model.id = Set(Uuid::new_v4());
            }
            model.created_at = Set(chrono::Utc::now().naive_utc());
        }
        Ok(model)
    }
}
