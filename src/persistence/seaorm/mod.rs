//! Implementación SeaORM de la frontera de persistencia.
//!
//! TODO el acoplamiento a SeaORM del dominio `User` vive en este módulo
//! (`entity` = mapeo de tabla, `support` = helpers de transacción/paginación).
//! Para portar a otro ORM, duplica este directorio (`persistence/welds/`, etc.)
//! implementando los mismos traits y cámbialo en `bootstrap`. Nada más se toca.

pub mod entity;
mod support;

// Helpers SeaORM reutilizables por apps que definan SUS propias entidades
// (transacción atómica + paginación tipada). Públicos aquí, NO en `core`.
pub use self::support::{AtomicFlow, Paginable};

use self::entity::{self as user, Entity as UserEntity};
use super::{Datastore, NewUser, RepoError, RepoResult, User, UserRepository};
use crate::core::query::{PageParams, PaginatedResult};
use async_trait::async_trait;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, Condition, DatabaseConnection, DbErr, EntityTrait,
    PaginatorTrait, QueryFilter, QueryOrder, Set,
};
use std::sync::Arc;

/// Traducción del error nativo del ORM al error agnóstico del dominio.
impl From<DbErr> for RepoError {
    fn from(e: DbErr) -> Self {
        RepoError::Backend(e.to_string())
    }
}

/// Mapeo entidad-de-persistencia → entidad-de-dominio.
impl From<user::Model> for User {
    fn from(m: user::Model) -> Self {
        User {
            id: m.id,
            username: m.username,
            email: m.email,
            password_hash: m.password_hash,
            created_at: m.created_at,
        }
    }
}

pub struct SeaOrmUserRepository {
    db: Arc<DatabaseConnection>,
}

impl SeaOrmUserRepository {
    pub fn new(db: Arc<DatabaseConnection>) -> Self {
        Self { db }
    }
}

#[async_trait]
impl UserRepository for SeaOrmUserRepository {
    async fn exists(&self, username: &str, email: &str) -> RepoResult<bool> {
        let count = UserEntity::find()
            .filter(
                Condition::any()
                    .add(user::Column::Username.eq(username))
                    .add(user::Column::Email.eq(email)),
            )
            .count(self.db.as_ref())
            .await?;
        Ok(count > 0)
    }

    async fn find_by_username(&self, username: &str) -> RepoResult<Option<User>> {
        let found = UserEntity::find()
            .filter(user::Column::Username.eq(username))
            .one(self.db.as_ref())
            .await?;
        Ok(found.map(User::from))
    }

    async fn insert(&self, new_user: NewUser) -> RepoResult<User> {
        let NewUser {
            username,
            email,
            password_hash,
        } = new_user;
        // FnOnce + `move`: trasladamos los strings al closure y al future sin
        // clonar. `before_save` (ActiveModelBehavior) asigna id/created_at.
        let model = AtomicFlow::run(self.db.as_ref(), move |txn| {
            Box::pin(async move {
                user::ActiveModel {
                    username: Set(username),
                    email: Set(email),
                    password_hash: Set(password_hash),
                    ..Default::default()
                }
                .insert(txn)
                .await
            })
        })
        .await?;
        Ok(User::from(model))
    }

    async fn find_all(&self, params: &PageParams) -> RepoResult<PaginatedResult<User>> {
        let paginator = UserEntity::find()
            .order_by_desc(user::Column::CreatedAt)
            .paginate_custom(self.db.as_ref(), params);
        let total = paginator.num_items().await?;
        let pages = paginator.num_pages().await?;
        let data = paginator.fetch_page(params.page.saturating_sub(1)).await?;
        Ok(PaginatedResult {
            data: data.into_iter().map(User::from).collect(),
            total,
            page: params.page,
            pages,
        })
    }
}

pub struct SeaOrmDatastore {
    db: Arc<DatabaseConnection>,
}

impl SeaOrmDatastore {
    pub fn new(db: Arc<DatabaseConnection>) -> Self {
        Self { db }
    }
}

#[async_trait]
impl Datastore for SeaOrmDatastore {
    async fn ping(&self) -> bool {
        self.db.as_ref().ping().await.is_ok()
    }
}
