use crate::auth::Claims;
use crate::core::blackbox::FlightRecord;
use crate::core::error::AppError;
use crate::core::monitor::MonitorSnapshot;
use crate::core::query::{PageParams, PaginatedResult};
use crate::core::validation::{validate_email, validate_password, validate_username};
use crate::models::user;
use crate::services::recovery_service::RecoveryService;
use crate::services::user_service::UserService;
use crate::state::AppState;
use actix_web::{
    dev::Payload,
    error::{ErrorForbidden, ErrorUnauthorized},
    http::StatusCode,
    web, FromRequest, HttpRequest, HttpResponse, Responder, ResponseError,
};
use serde::Serialize;
use std::future::Future;
use std::pin::Pin;
use utoipa::OpenApi;

// --- Cuarentena de Actix --------------------------------------------------
// Todo el acoplamiento HTTP (extractores y mapeo de errores) vive AQUÍ, en la
// capa `api`. El dominio (`auth::Claims`, `core::AppError`) es transport-agnostic.

/// Mapeo de `AppError` (dominio) → respuesta HTTP. `ResponseError` es de Actix;
/// el `impl` para un tipo del crate es legal por la regla de orfandad.
#[derive(Serialize)]
struct ErrorResponse {
    code: u16,
    message: String,
}

impl ResponseError for AppError {
    fn status_code(&self) -> StatusCode {
        match self {
            AppError::DbError(_) => StatusCode::INTERNAL_SERVER_ERROR,
            AppError::ValidationError(_) => StatusCode::BAD_REQUEST,
            AppError::NotFound => StatusCode::NOT_FOUND,
            AppError::Conflict(_) => StatusCode::CONFLICT,
            AppError::RateLimited => StatusCode::TOO_MANY_REQUESTS,
        }
    }

    fn error_response(&self) -> HttpResponse {
        let status = self.status_code();
        HttpResponse::build(status).json(ErrorResponse {
            code: status.as_u16(),
            message: self.to_string(),
        })
    }
}

/// Extractor: exige un access token JWT válido y no revocado.
impl FromRequest for Claims {
    type Error = actix_web::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self, Self::Error>>>>;

    fn from_request(req: &HttpRequest, _payload: &mut Payload) -> Self::Future {
        let req = req.clone();
        Box::pin(async move {
            let state = req
                .app_data::<web::Data<AppState>>()
                .ok_or_else(|| ErrorUnauthorized("App state not found"))?;

            let auth_header = req.headers().get("Authorization");

            match auth_header {
                Some(auth_val) => {
                    let auth_str = auth_val.to_str().unwrap_or("");
                    if !auth_str.starts_with("Bearer ") {
                        return Err(ErrorUnauthorized("Invalid token format"));
                    }
                    let token = &auth_str[7..];

                    let claims = state
                        .auth
                        .verify_token(token)
                        .await
                        .map_err(|_| ErrorUnauthorized("Invalid or expired token"))?;

                    // Solo los access tokens autorizan endpoints; un refresh
                    // token no debe usarse como credencial de API.
                    if !claims.is_access() {
                        return Err(ErrorUnauthorized(
                            "Refresh tokens cannot be used to access APIs",
                        ));
                    }
                    // Denylist: tokens revocados (logout) dejan de valer.
                    if !claims.jti.is_empty() && state.revocations.is_revoked(&claims.jti).await {
                        return Err(ErrorUnauthorized("Token has been revoked"));
                    }
                    Ok(claims)
                }
                None => Err(ErrorUnauthorized("Missing Authorization header")),
            }
        })
    }
}

/// Extractor que exige un JWT válido **con el rol `admin`**.
/// Las rutas administrativas deben usar este extractor en lugar de `Claims`.
pub struct AdminClaims(pub Claims);

impl FromRequest for AdminClaims {
    type Error = actix_web::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self, Self::Error>>>>;

    fn from_request(req: &HttpRequest, payload: &mut Payload) -> Self::Future {
        let claims_fut = Claims::from_request(req, payload);
        Box::pin(async move {
            let claims = claims_fut.await?;
            if claims.has_role("admin") {
                Ok(AdminClaims(claims))
            } else {
                Err(ErrorForbidden("Admin role required"))
            }
        })
    }
}
// --- fin cuarentena -------------------------------------------------------

#[derive(OpenApi)]
#[openapi(
    paths(login, refresh, logout, create_user, list_users, health_check, metrics, get_monitor, list_failed_jobs, replay_job),
    components(schemas(user::Model, LoginRequest, CreateUserRequest, RefreshRequest, PaginatedResult<user::Model>, MonitorSnapshot, FlightRecord)),
    security(("jwt_auth" = []))
)]
pub struct ApiDoc;

/// IP del cliente respetando proxies (X-Forwarded-For vía connection_info).
fn client_ip(req: &HttpRequest) -> String {
    req.connection_info()
        .realip_remote_addr()
        .unwrap_or("unknown")
        .to_string()
}

#[derive(serde::Deserialize, utoipa::ToSchema)]
pub struct LoginRequest {
    #[schema(example = "engineer_one")]
    pub username: String,
    #[schema(example = "s3cr3t-passphrase")]
    pub password: String,
}

#[derive(serde::Deserialize, utoipa::ToSchema)]
pub struct CreateUserRequest {
    #[schema(example = "engineer_one")]
    pub username: String,
    #[schema(example = "admin@example.com")]
    pub email: String,
    #[schema(example = "s3cr3t-passphrase")]
    pub password: String,
}

#[derive(serde::Deserialize, utoipa::ToSchema)]
pub struct RefreshRequest {
    pub refresh_token: String,
}

/// Deriva los roles a partir de la allowlist `PULSE_ADMIN_USERS`
/// (lista separada por comas). Por defecto todo usuario es `user`;
/// nunca se concede `admin` automáticamente.
///
/// NOTA: los roles se hornean en el JWT en el momento de emisión. Cambiar
/// `PULSE_ADMIN_USERS` solo afecta a tokens NUEVOS; un token ya emitido conserva
/// sus roles hasta expirar (o hasta revocarlo vía logout/denylist).
pub(crate) fn resolve_roles(username: &str) -> Vec<String> {
    let is_admin = std::env::var("PULSE_ADMIN_USERS")
        .unwrap_or_default()
        .split(',')
        .map(|s| s.trim())
        .any(|s| !s.is_empty() && s == username);
    if is_admin {
        vec!["admin".to_string(), "user".to_string()]
    } else {
        vec!["user".to_string()]
    }
}

pub fn config(cfg: &mut web::ServiceConfig) {
    cfg.service(
        web::scope("/api/v1")
            .route("/auth/login", web::post().to(login))
            .route("/auth/refresh", web::post().to(refresh))
            .route("/auth/logout", web::post().to(logout))
            .route("/users", web::post().to(create_user))
            .route("/users", web::get().to(list_users))
            .route("/health", web::get().to(health_check))
            .route("/metrics", web::get().to(metrics))
            .route("/admin/monitor", web::get().to(get_monitor))
            .route("/admin/morgue", web::get().to(list_failed_jobs))
            .route("/admin/replay/{id}", web::post().to(replay_job)),
    );
}

#[utoipa::path(post, path = "/api/v1/auth/login", request_body = LoginRequest, responses((status = 200, description = "Access + refresh tokens"), (status = 429, description = "Rate limited")))]
async fn login(
    req: HttpRequest,
    state: web::Data<AppState>,
    body: web::Json<LoginRequest>,
) -> impl Responder {
    // Anti fuerza bruta: por IP, ventana definida al construir el limiter.
    if !state
        .rate_limiter
        .check(&format!("login:{}", client_ip(&req)))
        .await
    {
        return HttpResponse::TooManyRequests()
            .json(serde_json::json!({ "error": "too many login attempts" }));
    }

    // El "método de login" es enchufable (BD por defecto, AD/LDAP/otra tabla si
    // la app inyecta su propio `Authenticator`). Tras verificar credenciales,
    // Pulse emite SU token con los roles que devuelva el authenticator.
    match state
        .authenticator
        .authenticate(&state, &body.username, &body.password)
        .await
    {
        Some(outcome) => {
            let user_id = outcome.user_id;
            let access = state
                .auth
                .create_token(&user_id, outcome.roles.clone())
                .await;
            let refresh_tok = state.auth.create_refresh_token(&user_id, outcome.roles).await;
            match (access, refresh_tok) {
                (Ok(access), Ok(refresh_tok)) => HttpResponse::Ok().json(serde_json::json!({
                    // "token" se mantiene por compatibilidad con clientes previos.
                    "token": access,
                    "access_token": access,
                    "refresh_token": refresh_tok,
                    "token_type": "Bearer"
                })),
                _ => HttpResponse::InternalServerError().finish(),
            }
        }
        None => HttpResponse::Unauthorized().finish(),
    }
}

#[utoipa::path(post, path = "/api/v1/auth/refresh", request_body = RefreshRequest, responses((status = 200, description = "New access token")))]
async fn refresh(state: web::Data<AppState>, body: web::Json<RefreshRequest>) -> impl Responder {
    match state.auth.verify_token(&body.refresh_token).await {
        Ok(claims) if claims.is_refresh() => {
            if !claims.jti.is_empty() && state.revocations.is_revoked(&claims.jti).await {
                return HttpResponse::Unauthorized()
                    .json(serde_json::json!({ "error": "refresh token revoked" }));
            }
            match state
                .auth
                .create_token(&claims.sub, claims.roles.clone())
                .await
            {
                Ok(access) => HttpResponse::Ok().json(serde_json::json!({
                    "token": access,
                    "access_token": access,
                    "token_type": "Bearer"
                })),
                Err(_) => HttpResponse::InternalServerError().finish(),
            }
        }
        _ => HttpResponse::Unauthorized()
            .json(serde_json::json!({ "error": "invalid refresh token" })),
    }
}

#[utoipa::path(post, path = "/api/v1/auth/logout", security(("jwt_auth" = [])), responses((status = 200, description = "Token revoked")))]
async fn logout(state: web::Data<AppState>, claims: Claims) -> impl Responder {
    // Revoca el access token actual durante su TTL restante.
    state
        .revocations
        .revoke(&claims.jti, claims.remaining_ttl_secs())
        .await;
    HttpResponse::Ok().json(serde_json::json!({ "status": "logged_out" }))
}

#[utoipa::path(get, path = "/api/v1/users", params(PageParams), security(("jwt_auth" = [])), responses((status = 200, body = PaginatedResult<user::Model>)))]
async fn list_users(
    state: web::Data<AppState>,
    info: web::Query<PageParams>,
    _auth: Claims,
) -> Result<impl Responder, AppError> {
    let result = UserService::find_all(&state, info.into_inner())
        .await
        .map_err(AppError::from)?;
    Ok(HttpResponse::Ok().json(result))
}

#[utoipa::path(post, path = "/api/v1/users", request_body = CreateUserRequest, responses((status = 201, body = user::Model), (status = 400, description = "Validation error"), (status = 409, description = "Username/email taken"), (status = 429, description = "Rate limited")))]
async fn create_user(
    http: HttpRequest,
    state: web::Data<AppState>,
    form: web::Json<CreateUserRequest>,
) -> impl Responder {
    // Registro público pero limitado por IP para evitar abuso/spam.
    if !state
        .rate_limiter
        .check(&format!("register:{}", client_ip(&http)))
        .await
    {
        return AppError::RateLimited.error_response();
    }

    let data = form.into_inner();
    let username = data.username.trim().to_string();
    let email = data.email.trim().to_lowercase();

    // Validación de input (400) antes de tocar la BD.
    if let Err(msg) = validate_username(&username)
        .and_then(|_| validate_email(&email))
        .and_then(|_| validate_password(&data.password))
    {
        return AppError::ValidationError(msg).error_response();
    }

    // Pre-check de unicidad para devolver un 409 limpio (el índice UNIQUE de la
    // BD es el guardián real ante condiciones de carrera).
    match UserService::exists(&state, &username, &email).await {
        Ok(true) => {
            return AppError::Conflict("username or email already in use".into()).error_response()
        }
        Ok(false) => {}
        // No pudimos verificar unicidad (p.ej. BD caída): NO cortamos con 500.
        // Dejamos que el INSERT falle más abajo y se capture en el blackbox
        // (preserva la recuperación / replay). El índice UNIQUE sigue siendo el
        // guardián real ante duplicados.
        Err(e) => tracing::warn!("uniqueness pre-check failed ({e}); proceeding to insert"),
    }

    // El payload del blackbox NUNCA debe contener la contraseña en claro.
    let payload = serde_json::json!({ "username": username, "email": email });

    match UserService::create_user(&state, username, email, data.password).await {
        Ok(user) => HttpResponse::Created().json(user),
        Err(e) => {
            state
                .monitor
                .failures_total
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            RecoveryService::capture_failure(&state, "create_user", payload, e.to_string()).await;
            let app_error = AppError::from(e);
            app_error.error_response()
        }
    }
}

#[utoipa::path(get, path = "/api/v1/health", responses((status = 200, description = "Operational"), (status = 503, description = "Degraded")))]
async fn health_check(state: web::Data<AppState>) -> impl Responder {
    let db_ok = state.db.as_ref().ping().await.is_ok();
    // None = backend remoto no configurado (modo local) → no penaliza la salud.
    let backend_health = state.store.health().await;
    let backend_ok = backend_health.unwrap_or(true);
    let local_cache_size = state.store.local_count();

    let body = serde_json::json!({
        "status": if db_ok && backend_ok { "operational" } else { "degraded" },
        "checks": {
            "database": if db_ok { "up" } else { "down" },
            "backend": match backend_health {
                Some(true) => "up",
                Some(false) => "down",
                None => "disabled",
            },
        },
        "local_cache_entries": local_cache_size,
    });

    if db_ok && backend_ok {
        HttpResponse::Ok().json(body)
    } else {
        HttpResponse::ServiceUnavailable().json(body)
    }
}

#[utoipa::path(get, path = "/api/v1/metrics", responses((status = 200, description = "Prometheus exposition format")))]
async fn metrics(state: web::Data<AppState>) -> impl Responder {
    let s = state.monitor.snapshot();
    let body = format!(
        "# HELP pulse_uptime_seconds Process uptime in seconds.\n\
         # TYPE pulse_uptime_seconds gauge\n\
         pulse_uptime_seconds {uptime}\n\
         # HELP pulse_requests_total Total HTTP requests handled.\n\
         # TYPE pulse_requests_total counter\n\
         pulse_requests_total {requests}\n\
         # HELP pulse_failures_total Total handler failures.\n\
         # TYPE pulse_failures_total counter\n\
         pulse_failures_total {failures}\n\
         # HELP pulse_active_connections Currently active connections.\n\
         # TYPE pulse_active_connections gauge\n\
         pulse_active_connections {active}\n\
         # HELP pulse_app_ram_mb Resident memory of the process in MB.\n\
         # TYPE pulse_app_ram_mb gauge\n\
         pulse_app_ram_mb {app_ram}\n\
         # HELP pulse_system_ram_mb System-wide used memory in MB.\n\
         # TYPE pulse_system_ram_mb gauge\n\
         pulse_system_ram_mb {sys_ram}\n\
         # HELP pulse_cpu_usage_percent Global CPU usage percent.\n\
         # TYPE pulse_cpu_usage_percent gauge\n\
         pulse_cpu_usage_percent {cpu}\n\
         # HELP pulse_local_cache_entries Entries held in the L1 cache.\n\
         # TYPE pulse_local_cache_entries gauge\n\
         pulse_local_cache_entries {cache}\n",
        uptime = s.uptime_seconds,
        requests = s.total_requests,
        failures = s.total_failures,
        active = s.current_active,
        app_ram = s.ram_usage_mb,
        sys_ram = s.system_ram_mb,
        cpu = s.cpu_usage,
        cache = state.store.local_count(),
    );
    HttpResponse::Ok()
        .content_type("text/plain; version=0.0.4; charset=utf-8")
        .body(body)
}

#[utoipa::path(get, path = "/api/v1/admin/monitor", responses((status = 200, body = MonitorSnapshot)))]
async fn get_monitor(state: web::Data<AppState>, _admin: AdminClaims) -> impl Responder {
    HttpResponse::Ok().json(state.monitor.snapshot())
}

#[utoipa::path(get, path = "/api/v1/admin/morgue", responses((status = 200, body = Vec<FlightRecord>)))]
async fn list_failed_jobs(state: web::Data<AppState>, _admin: AdminClaims) -> impl Responder {
    let failures = RecoveryService::list_failures(&state).await;
    HttpResponse::Ok().json(failures)
}

#[utoipa::path(post, path = "/api/v1/admin/replay/{id}", responses((status = 200, description = "Replay OK")))]
async fn replay_job(
    state: web::Data<AppState>,
    id: web::Path<String>,
    _admin: AdminClaims,
) -> impl Responder {
    match RecoveryService::replay_from_disk(&state, &id).await {
        Ok(msg) => {
            HttpResponse::Ok().json(serde_json::json!({ "status": "restored", "info": msg }))
        }
        Err(e) => HttpResponse::BadRequest().json(serde_json::json!({ "error": e })),
    }
}
