pub mod recovery_service;
pub mod user_service;

/// Frontera de persistencia: traduce el error del ORM (SeaORM) al `AppError`
/// agnóstico. Vive aquí, no en `core::error`, para que el tipo de error público
/// no dependa de SeaORM (igual que `HybridError` no depende de Redis). Habilita
/// `?` sobre `DbErr` en handlers que devuelven `Result<_, AppError>`.
impl From<sea_orm::DbErr> for crate::core::error::AppError {
    fn from(e: sea_orm::DbErr) -> Self {
        crate::core::error::AppError::DbError(e.to_string())
    }
}
