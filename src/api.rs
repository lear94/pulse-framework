use crate::auth::Claims;
use crate::core::blackbox::FlightRecord;
use crate::core::error::AppError;
use crate::core::monitor::MonitorSnapshot;
use crate::core::query::{PageParams, PaginatedResult};
use crate::models::user;
use crate::services::recovery_service::RecoveryService;
use crate::services::user_service::UserService;
use crate::state::AppState;
use actix_web::{web, HttpResponse, Responder, ResponseError};
use utoipa::OpenApi;

#[derive(OpenApi)]
#[openapi(
    paths(login, create_user, list_users, health_check, get_monitor, list_failed_jobs, replay_job),
    components(schemas(user::Model, LoginRequest, PaginatedResult<user::Model>, MonitorSnapshot, FlightRecord)),
    security(("jwt_auth" = []))
)]
pub struct ApiDoc;

#[derive(serde::Deserialize, utoipa::ToSchema)]
pub struct LoginRequest {
    #[schema(example = "engineer_one")]
    pub username: String,
}

pub fn config(cfg: &mut web::ServiceConfig) {
    cfg.service(
        web::scope("/api/v1")
            .route("/auth/login", web::post().to(login))
            .route("/users", web::post().to(create_user))
            .route("/users", web::get().to(list_users))
            .route("/health", web::get().to(health_check))
            .route("/admin/monitor", web::get().to(get_monitor))
            .route("/admin/morgue", web::get().to(list_failed_jobs))
            .route("/admin/replay/{id}", web::post().to(replay_job)),
    );
}

#[utoipa::path(post, path = "/api/v1/auth/login", request_body = LoginRequest, responses((status = 200, description = "JWT Token")))]
async fn login(state: web::Data<AppState>, body: web::Json<LoginRequest>) -> impl Responder {
    match UserService::login(&state, body.username.clone()).await {
        Some(user_id) => {
            let roles = vec!["admin".to_string()];
            match state.auth.create_token(&user_id, roles).await {
                Ok(token) => HttpResponse::Ok().json(serde_json::json!({ "token": token })),
                Err(_) => HttpResponse::InternalServerError().finish(),
            }
        }
        None => HttpResponse::Unauthorized().finish(),
    }
}

#[utoipa::path(get, path = "/api/v1/users", params(PageParams), security(("jwt_auth" = [])), responses((status = 200, body = PaginatedResult<user::Model>)))]
async fn list_users(
    state: web::Data<AppState>,
    info: web::Query<PageParams>,
    _auth: Claims,
) -> Result<impl Responder, AppError> {
    let result = UserService::find_all(&state, info.into_inner())
        .await
        .map_err(AppError::DbError)?;
    Ok(HttpResponse::Ok().json(result))
}

#[utoipa::path(post, path = "/api/v1/users", request_body = user::Model, responses((status = 201, body = user::Model)))]
async fn create_user(state: web::Data<AppState>, form: web::Json<user::Model>) -> impl Responder {
    state
        .monitor
        .requests_total
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let original_form = form.into_inner();
    let payload = serde_json::to_value(&original_form).unwrap_or(serde_json::Value::Null);

    match UserService::create_user(&state, original_form).await {
        Ok(user) => HttpResponse::Created().json(user),
        Err(e) => {
            state
                .monitor
                .failures_total
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            RecoveryService::capture_failure(&state, "create_user", payload, e.to_string()).await;
            let app_error = AppError::DbError(e);
            app_error.error_response()
        }
    }
}

#[utoipa::path(get, path = "/api/v1/health", responses((status = 200, description = "Operational")))]
async fn health_check(state: web::Data<AppState>) -> impl Responder {
    let db_health = state.db.as_ref().ping().await;
    let local_cache_size = state.store.local_count();
    match db_health {
        Ok(_) => HttpResponse::Ok().json(
            serde_json::json!({ "status": "operational", "local_cache_entries": local_cache_size }),
        ),
        Err(_) => {
            HttpResponse::ServiceUnavailable().json(serde_json::json!({ "status": "degraded" }))
        }
    }
}

#[utoipa::path(get, path = "/api/v1/admin/monitor", responses((status = 200, body = MonitorSnapshot)))]
async fn get_monitor(state: web::Data<AppState>) -> impl Responder {
    HttpResponse::Ok().json(state.monitor.snapshot())
}

#[utoipa::path(get, path = "/api/v1/admin/morgue", responses((status = 200, body = Vec<FlightRecord>)))]
async fn list_failed_jobs(state: web::Data<AppState>) -> impl Responder {
    let failures = RecoveryService::list_failures(&state).await;
    HttpResponse::Ok().json(failures)
}

#[utoipa::path(post, path = "/api/v1/admin/replay/{id}", responses((status = 200, description = "Replay OK")))]
async fn replay_job(state: web::Data<AppState>, id: web::Path<String>) -> impl Responder {
    match RecoveryService::replay_from_disk(&state, &id).await {
        Ok(msg) => {
            HttpResponse::Ok().json(serde_json::json!({ "status": "restored", "info": msg }))
        }
        Err(e) => HttpResponse::BadRequest().json(serde_json::json!({ "error": e })),
    }
}
