use async_trait::async_trait;
use sea_orm::entity::prelude::*;
use sea_orm::ActiveValue::{NotSet, Set};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize, ToSchema)]
#[sea_orm(table_name = "users")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    #[serde(default)]
    pub id: Uuid,
    #[schema(example = "engineer_one")]
    pub username: String,
    #[schema(example = "admin@example.com")]
    pub email: String,
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
        if let Set(ref mut email) = model.email {
            *email = email.to_lowercase();
        }
        if insert {
            if let NotSet = model.id {
                model.id = Set(Uuid::new_v4());
            }
            model.created_at = Set(chrono::Utc::now().naive_utc());
        }
        Ok(model)
    }
}
