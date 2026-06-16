pub mod recovery_service;
pub mod user_service;

/// Traduce el error agnóstico de persistencia (`RepoError`, sin dependencia de
/// ningún ORM) al `AppError` público. Habilita `?`/`map_err(AppError::from)` en
/// handlers que devuelven `Result<_, AppError>`.
impl From<crate::persistence::RepoError> for crate::core::error::AppError {
    fn from(e: crate::persistence::RepoError) -> Self {
        use crate::core::error::AppError;
        use crate::persistence::RepoError;
        match e {
            RepoError::NotFound => AppError::NotFound,
            RepoError::Conflict => AppError::Conflict("unique constraint violation".into()),
            RepoError::Backend(msg) => AppError::DbError(msg),
        }
    }
}
