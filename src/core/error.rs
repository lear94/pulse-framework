use actix_web::{http::StatusCode, HttpResponse, ResponseError};
use serde::Serialize;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum AppError {
    #[error("Database error: {0}")]
    DbError(#[from] sea_orm::DbErr),
    #[error("Authentication failed: {0}")]
    AuthError(String),
    #[error("Validation error: {0}")]
    ValidationError(String),
    #[error("Resource not found")]
    NotFound,
    #[error("Conflict: {0}")]
    Conflict(String),
    #[error("Too many requests")]
    RateLimited,
    #[error("System overload")]
    SystemOverload,
}

#[derive(Serialize)]
struct ErrorResponse {
    code: u16,
    message: String,
}

impl ResponseError for AppError {
    fn status_code(&self) -> StatusCode {
        match self {
            AppError::DbError(_) => StatusCode::INTERNAL_SERVER_ERROR,
            AppError::AuthError(_) => StatusCode::UNAUTHORIZED,
            AppError::ValidationError(_) => StatusCode::BAD_REQUEST,
            AppError::NotFound => StatusCode::NOT_FOUND,
            AppError::Conflict(_) => StatusCode::CONFLICT,
            AppError::RateLimited => StatusCode::TOO_MANY_REQUESTS,
            AppError::SystemOverload => StatusCode::TOO_MANY_REQUESTS,
        }
    }

    fn error_response(&self) -> HttpResponse {
        let status = self.status_code();
        let response = ErrorResponse {
            code: status.as_u16(),
            message: self.to_string(),
        };
        HttpResponse::build(status).json(response)
    }
}
