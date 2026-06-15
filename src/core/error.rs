use thiserror::Error;

/// Error de dominio del framework. Agnóstico al transporte: el mapeo a HTTP
/// (`ResponseError`) vive en la capa `api`, no aquí.
#[derive(Error, Debug)]
pub enum AppError {
    #[error("Database error: {0}")]
    DbError(#[from] sea_orm::DbErr),
    #[error("Validation error: {0}")]
    ValidationError(String),
    #[error("Resource not found")]
    NotFound,
    #[error("Conflict: {0}")]
    Conflict(String),
    #[error("Too many requests")]
    RateLimited,
}
