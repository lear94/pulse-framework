//! Rutas HTTP de la wiki + frontend embebido.
//!
//! Demuestra cómo una app downstream:
//!   - registra sus rutas junto a las del core en el mismo `ServiceConfig`,
//!   - protege escrituras con el extractor `Claims` (JWT del framework),
//!   - reutiliza `AppError` (mapeo a 400/404/409/500) y la paginación del core,
//!   - usa la caché híbrida L1/L2 (`state.store`) con invalidación explícita.

use crate::model;
use crate::revision;
use crate::service::PageService;
use actix_web::{web, HttpResponse, Responder};
use pulse_core::auth::Claims;
use pulse_core::core::error::AppError;
use pulse_core::core::query::{PageParams, PaginatedResult};
use pulse_core::persistence::RepoError;
use pulse_core::state::AppState;
use sea_orm::{DatabaseConnection, DbErr, EntityTrait};
use serde::Deserialize;
use utoipa::{OpenApi, ToSchema};

/// La conexión es PROPIA de la app: tras desacoplar el ORM, el framework ya no
/// expone su pool por `AppState`. La inyectamos como `app_data` en `configure`.
/// El `DbErr` del ORM se traduce a `AppError` por la ruta agnóstica del core
/// (`DbErr → RepoError → AppError`), sin acoplar la wiki a un mapeo propio.
fn db_err(e: DbErr) -> AppError {
    AppError::from(RepoError::from(e))
}

/// HTML del frontend (SPA en vanilla JS). Se sirve en `/`.
const INDEX_HTML: &str = include_str!("../static/index.html");

#[derive(OpenApi)]
#[openapi(
    paths(
        list_pages, get_page, create_page, update_page, delete_page, search_pages,
        list_revisions, get_revision, restore_revision
    ),
    components(schemas(
        model::Model,
        revision::Model,
        CreatePageRequest,
        UpdatePageRequest,
        PaginatedResult<model::Model>
    )),
    security(("jwt_auth" = []))
)]
pub struct WikiApiDoc;

#[derive(Deserialize, ToSchema)]
pub struct CreatePageRequest {
    #[schema(example = "Getting Started")]
    pub title: String,
    #[schema(example = "# Welcome\n\nWrite **markdown** here.")]
    pub content: String,
    /// Slug opcional; si se omite se deriva del título.
    #[schema(example = "getting-started")]
    pub slug: Option<String>,
}

#[derive(Deserialize, ToSchema)]
pub struct UpdatePageRequest {
    pub title: Option<String>,
    pub content: Option<String>,
}

const MAX_TITLE: usize = 200;
const MAX_CONTENT: usize = 100_000;

/// Registra TODAS las rutas: las del core (auth/users/health/metrics/admin), las
/// de la wiki bajo `/api/wiki`, y el frontend en `/`.
pub fn configure(cfg: &mut web::ServiceConfig, db: web::Data<DatabaseConnection>) {
    // La conexión propia de la wiki, disponible para sus handlers vía extractor.
    cfg.app_data(db);

    // Rutas del framework (login/refresh/logout, users, health, metrics, admin…).
    pulse_core::api::config(cfg);

    // Rutas de la wiki en un prefijo propio que NO solapa con `/api/v1` del core.
    cfg.service(
        web::scope("/api/wiki")
            .route("/pages", web::get().to(list_pages))
            .route("/pages", web::post().to(create_page))
            .route("/pages/{slug}", web::get().to(get_page))
            .route("/pages/{slug}", web::put().to(update_page))
            .route("/pages/{slug}", web::delete().to(delete_page))
            .route("/pages/{slug}/revisions", web::get().to(list_revisions))
            .route("/pages/{slug}/revisions/{rev}", web::get().to(get_revision))
            .route(
                "/pages/{slug}/revisions/{rev}/restore",
                web::post().to(restore_revision),
            )
            .route("/search", web::get().to(search_pages)),
    );

    // Frontend.
    cfg.route("/", web::get().to(index));
}

async fn index() -> impl Responder {
    HttpResponse::Ok()
        .content_type("text/html; charset=utf-8")
        .body(INDEX_HTML)
}

fn cache_key(slug: &str) -> String {
    format!("wiki:page:{slug}")
}

/// Convierte un texto a slug url-friendly: minúsculas, alfanumérico y guiones.
fn slugify(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut prev_dash = false;
    for c in input.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            prev_dash = false;
        } else if !out.is_empty() && !prev_dash {
            out.push('-');
            prev_dash = true;
        }
    }
    out.trim_matches('-').to_string()
}

/// Resuelve el nombre de autor a partir del `sub` (uuid) del JWT.
async fn resolve_author(db: &DatabaseConnection, claims: &Claims) -> String {
    if let Ok(uid) = uuid::Uuid::parse_str(&claims.sub) {
        if let Ok(Some(u)) = pulse_core::persistence::seaorm::entity::Entity::find_by_id(uid)
            .one(db)
            .await
        {
            return u.username;
        }
    }
    claims.sub.clone()
}

#[utoipa::path(get, path = "/api/wiki/pages", params(PageParams),
    responses((status = 200, body = PaginatedResult<model::Model>)))]
async fn list_pages(
    db: web::Data<DatabaseConnection>,
    query: web::Query<PageParams>,
) -> Result<impl Responder, AppError> {
    let result = PageService::list(db.as_ref(), &query.into_inner())
        .await
        .map_err(db_err)?;
    Ok(HttpResponse::Ok().json(result))
}

#[utoipa::path(get, path = "/api/wiki/pages/{slug}",
    responses((status = 200, body = model::Model), (status = 404, description = "Not found")))]
async fn get_page(
    state: web::Data<AppState>,
    db: web::Data<DatabaseConnection>,
    slug: web::Path<String>,
) -> Result<impl Responder, AppError> {
    let slug = slug.into_inner();
    let key = cache_key(&slug);

    // 1) Caché (L1 dashmap / L2 redis si está configurado).
    if let Some(cached) = state.store.get::<model::Model>(&key).await {
        return Ok(HttpResponse::Ok().json(cached));
    }

    // 2) Base de datos.
    let page = PageService::get_by_slug(db.as_ref(), &slug)
        .await
        .map_err(db_err)?
        .ok_or(AppError::NotFound)?;

    // 3) Repuebla la caché (best-effort: un fallo de caché no rompe la lectura).
    if let Err(e) = state.store.set(&key, &page).await {
        pulse_core::tracing::warn!("wiki cache set failed for {slug}: {e}");
    }
    Ok(HttpResponse::Ok().json(page))
}

#[utoipa::path(post, path = "/api/wiki/pages", request_body = CreatePageRequest,
    security(("jwt_auth" = [])),
    responses((status = 201, body = model::Model), (status = 400, description = "Validation error"),
        (status = 401, description = "Unauthorized"), (status = 409, description = "Slug taken")))]
async fn create_page(
    state: web::Data<AppState>,
    db: web::Data<DatabaseConnection>,
    body: web::Json<CreatePageRequest>,
    auth: Claims,
) -> Result<impl Responder, AppError> {
    let data = body.into_inner();
    let title = data.title.trim().to_string();
    let content = data.content;

    validate(&title, &content)?;

    let slug = match data.slug {
        Some(s) if !slugify(&s).is_empty() => slugify(&s),
        _ => slugify(&title),
    };
    if slug.is_empty() {
        return Err(AppError::ValidationError(
            "could not derive a slug from the title; provide one explicitly".into(),
        ));
    }

    if PageService::exists(db.as_ref(), &slug)
        .await
        .map_err(db_err)?
    {
        return Err(AppError::Conflict(format!("slug '{slug}' already exists")));
    }

    let author = resolve_author(db.as_ref(), &auth).await;
    let page = PageService::create(db.as_ref(), slug, title, content, author)
        .await
        .map_err(db_err)?;

    let _ = state.store.set(&cache_key(&page.slug), &page).await;
    Ok(HttpResponse::Created().json(page))
}

#[utoipa::path(put, path = "/api/wiki/pages/{slug}", request_body = UpdatePageRequest,
    security(("jwt_auth" = [])),
    responses((status = 200, body = model::Model), (status = 401, description = "Unauthorized"),
        (status = 404, description = "Not found")))]
async fn update_page(
    state: web::Data<AppState>,
    db: web::Data<DatabaseConnection>,
    slug: web::Path<String>,
    body: web::Json<UpdatePageRequest>,
    auth: Claims,
) -> Result<impl Responder, AppError> {
    let slug = slug.into_inner();
    let data = body.into_inner();

    // Valida solo los campos presentes.
    if let Some(t) = &data.title {
        if t.trim().is_empty() || t.len() > MAX_TITLE {
            return Err(AppError::ValidationError(format!(
                "title must be 1..={MAX_TITLE} chars"
            )));
        }
    }
    if let Some(c) = &data.content {
        if c.is_empty() || c.len() > MAX_CONTENT {
            return Err(AppError::ValidationError(format!(
                "content must be 1..={MAX_CONTENT} chars"
            )));
        }
    }

    let title = data.title.map(|t| t.trim().to_string());
    let editor = resolve_author(db.as_ref(), &auth).await;
    let updated = PageService::update(db.as_ref(), &slug, title, data.content, editor)
        .await
        .map_err(db_err)?
        .ok_or(AppError::NotFound)?;

    // Invalida la caché: la próxima lectura repuebla con el valor fresco.
    let _ = state.store.del(&cache_key(&slug)).await;
    Ok(HttpResponse::Ok().json(updated))
}

#[utoipa::path(delete, path = "/api/wiki/pages/{slug}",
    security(("jwt_auth" = [])),
    responses((status = 204, description = "Deleted"), (status = 401, description = "Unauthorized"),
        (status = 404, description = "Not found")))]
async fn delete_page(
    state: web::Data<AppState>,
    db: web::Data<DatabaseConnection>,
    slug: web::Path<String>,
    _auth: Claims,
) -> Result<impl Responder, AppError> {
    let slug = slug.into_inner();
    let deleted = PageService::delete(db.as_ref(), &slug)
        .await
        .map_err(db_err)?;
    if !deleted {
        return Err(AppError::NotFound);
    }
    let _ = state.store.del(&cache_key(&slug)).await;
    Ok(HttpResponse::NoContent().finish())
}

#[utoipa::path(get, path = "/api/wiki/search",
    params(("q" = String, Query, description = "Search term")),
    responses((status = 200, body = Vec<model::Model>)))]
async fn search_pages(
    db: web::Data<DatabaseConnection>,
    query: web::Query<SearchQuery>,
) -> Result<impl Responder, AppError> {
    let q = query.into_inner().q;
    if q.trim().is_empty() {
        return Ok(HttpResponse::Ok().json(Vec::<model::Model>::new()));
    }
    let results = PageService::search(db.as_ref(), q.trim())
        .await
        .map_err(db_err)?;
    Ok(HttpResponse::Ok().json(results))
}

#[utoipa::path(get, path = "/api/wiki/pages/{slug}/revisions",
    responses((status = 200, body = Vec<revision::Model>), (status = 404, description = "Not found")))]
async fn list_revisions(
    db: web::Data<DatabaseConnection>,
    slug: web::Path<String>,
) -> Result<impl Responder, AppError> {
    let page = PageService::get_by_slug(db.as_ref(), &slug)
        .await
        .map_err(db_err)?
        .ok_or(AppError::NotFound)?;
    let revisions = PageService::list_revisions(db.as_ref(), page.id)
        .await
        .map_err(db_err)?;
    Ok(HttpResponse::Ok().json(revisions))
}

#[utoipa::path(get, path = "/api/wiki/pages/{slug}/revisions/{rev}",
    responses((status = 200, body = revision::Model), (status = 404, description = "Not found")))]
async fn get_revision(
    db: web::Data<DatabaseConnection>,
    path: web::Path<(String, i32)>,
) -> Result<impl Responder, AppError> {
    let (slug, rev) = path.into_inner();
    let page = PageService::get_by_slug(db.as_ref(), &slug)
        .await
        .map_err(db_err)?
        .ok_or(AppError::NotFound)?;
    let revision = PageService::get_revision(db.as_ref(), page.id, rev)
        .await
        .map_err(db_err)?
        .ok_or(AppError::NotFound)?;
    Ok(HttpResponse::Ok().json(revision))
}

#[utoipa::path(post, path = "/api/wiki/pages/{slug}/revisions/{rev}/restore",
    security(("jwt_auth" = [])),
    responses((status = 200, body = model::Model), (status = 401, description = "Unauthorized"),
        (status = 404, description = "Not found")))]
async fn restore_revision(
    state: web::Data<AppState>,
    db: web::Data<DatabaseConnection>,
    path: web::Path<(String, i32)>,
    auth: Claims,
) -> Result<impl Responder, AppError> {
    let (slug, rev) = path.into_inner();
    let page = PageService::get_by_slug(db.as_ref(), &slug)
        .await
        .map_err(db_err)?
        .ok_or(AppError::NotFound)?;
    let snapshot = PageService::get_revision(db.as_ref(), page.id, rev)
        .await
        .map_err(db_err)?
        .ok_or(AppError::NotFound)?;

    // Restaurar = aplicar el contenido de la revisión como una nueva edición
    // (genera, a su vez, una nueva revisión). El historial nunca se reescribe.
    let editor = resolve_author(db.as_ref(), &auth).await;
    let updated = PageService::update(
        db.as_ref(),
        &slug,
        Some(snapshot.title),
        Some(snapshot.content),
        editor,
    )
    .await
    .map_err(db_err)?
    .ok_or(AppError::NotFound)?;

    let _ = state.store.del(&cache_key(&slug)).await;
    Ok(HttpResponse::Ok().json(updated))
}

#[derive(Deserialize)]
struct SearchQuery {
    #[serde(default)]
    q: String,
}

fn validate(title: &str, content: &str) -> Result<(), AppError> {
    if title.is_empty() || title.len() > MAX_TITLE {
        return Err(AppError::ValidationError(format!(
            "title must be 1..={MAX_TITLE} chars"
        )));
    }
    if content.is_empty() || content.len() > MAX_CONTENT {
        return Err(AppError::ValidationError(format!(
            "content must be 1..={MAX_CONTENT} chars"
        )));
    }
    Ok(())
}
