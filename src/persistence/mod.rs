//! Frontera de persistencia ORM-agnóstica.
//!
//! El dominio (servicios, handlers) habla SOLO con estos traits y tipos planos;
//! nunca con SeaORM. Cambiar de ORM (Welds/Tiberius/Rbatis/SeaORM X para SQL
//! Server, o seguir en Postgres) = reimplementar estos traits en un módulo nuevo
//! y enchufarlo en `bootstrap`. Cero cambios en servicios/API. Mismo patrón que
//! ya aplica el framework a Redis (`CacheBackend`), colas (`TaskQueue`) y auth
//! (`Authenticator`): la implementación concreta vive tras un `dyn Trait`.

pub mod seaorm;

use crate::core::query::{PageParams, PaginatedResult};
use async_trait::async_trait;
use chrono::NaiveDateTime;
use serde::{Deserialize, Serialize};
use std::fmt;
use utoipa::ToSchema;
use uuid::Uuid;

/// Entidad de dominio. Tipo plano (sin derivar de ningún ORM) que cruza la
/// frontera servicios/API. `password_hash` nunca se serializa a JSON.
#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
pub struct User {
    #[serde(default)]
    pub id: Uuid,
    #[schema(example = "engineer_one")]
    pub username: String,
    #[schema(example = "admin@example.com")]
    pub email: String,
    #[serde(default, skip_serializing)]
    pub password_hash: String,
    #[serde(skip_deserializing)]
    pub created_at: NaiveDateTime,
}

/// Datos para crear un usuario. La contraseña llega YA hasheada (el hashing es
/// lógica de dominio/CPU-bound del servicio, no de la capa de persistencia); el
/// `id`/`created_at` los asigna el repositorio.
pub struct NewUser {
    pub username: String,
    pub email: String,
    pub password_hash: String,
}

/// Error de persistencia agnóstico al ORM. La traducción desde el error nativo
/// (p.ej. `sea_orm::DbErr`) vive en cada implementación, no aquí, para que el
/// dominio no dependa del backend.
#[derive(Debug)]
pub enum RepoError {
    NotFound,
    Conflict,
    Backend(String),
}

impl fmt::Display for RepoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RepoError::NotFound => write!(f, "record not found"),
            RepoError::Conflict => write!(f, "unique constraint violation"),
            RepoError::Backend(e) => write!(f, "persistence backend error: {e}"),
        }
    }
}

impl std::error::Error for RepoError {}

pub type RepoResult<T> = Result<T, RepoError>;

/// Salud del almacén (health-check). Aísla el `ping` para que `AppState` no
/// exponga el pool concreto del ORM.
#[async_trait]
pub trait Datastore: Send + Sync {
    async fn ping(&self) -> bool;
}

/// Operaciones de persistencia del agregado `User`.
#[async_trait]
pub trait UserRepository: Send + Sync {
    /// ¿Existe ya un usuario con ese username o email? (pre-check de 409).
    async fn exists(&self, username: &str, email: &str) -> RepoResult<bool>;
    /// Carga el usuario por username (incluye `password_hash` para el login).
    async fn find_by_username(&self, username: &str) -> RepoResult<Option<User>>;
    /// Inserta y devuelve el usuario ya persistido (con id/created_at).
    async fn insert(&self, new_user: NewUser) -> RepoResult<User>;
    /// Página de usuarios ordenada por fecha de alta descendente.
    async fn find_all(&self, params: &PageParams) -> RepoResult<PaginatedResult<User>>;
}
