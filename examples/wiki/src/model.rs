//! Entidad `pages` — el modelo de datos de la wiki.
//!
//! Sigue el mismo patrón que `pulse_core::models::user`: una entidad sea-orm con
//! `ActiveModelBehavior` que rellena timestamps/uuid antes de guardar. Esto
//! demuestra que el framework no impone ningún modelo: la app define los suyos.

use async_trait::async_trait;
use sea_orm::entity::prelude::*;
use sea_orm::ActiveValue::{NotSet, Set};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize, ToSchema)]
#[sea_orm(table_name = "pages")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    #[serde(default)]
    pub id: Uuid,
    /// Identificador legible y único en la URL (ej. "getting-started").
    #[sea_orm(unique)]
    #[schema(example = "getting-started")]
    pub slug: String,
    #[schema(example = "Getting Started")]
    pub title: String,
    #[schema(example = "# Welcome\n\nThis is your first page.")]
    pub content: String,
    /// Username del autor (resuelto desde el JWT al crear la página).
    #[schema(example = "admin")]
    pub author: String,
    #[serde(skip_deserializing)]
    pub created_at: DateTime,
    #[serde(skip_deserializing)]
    pub updated_at: DateTime,
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
        let now = chrono::Utc::now().naive_utc();
        if insert {
            if let NotSet = model.id {
                model.id = Set(Uuid::new_v4());
            }
            model.created_at = Set(now);
        }
        // `updated_at` se refresca en cada guardado (insert o update).
        model.updated_at = Set(now);
        Ok(model)
    }
}
