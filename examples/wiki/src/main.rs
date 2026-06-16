//! Pulse Wiki — una wiki funcional construida sobre `pulse_core` como crate.
//!
//! Arranque:
//!   1. Garantiza el esquema (users, blackbox_records, pages) de forma idempotente.
//!   2. Siembra un usuario `admin` y una página de bienvenida (solo si faltan).
//!   3. Llama a `pulse_core::bootstrap(...)` registrando rutas del core + de la wiki.
//!
//! La app NO reimplementa auth, salud, métricas, caché ni shutdown: todo eso lo
//! aporta el framework. La wiki solo añade su entidad, su servicio y sus rutas.

use actix_web::web;
use pulse_core::persistence::seaorm::entity as core_user;
use pulse_core::{bootstrap, PulseConfig};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, Database, EntityTrait, QueryFilter, Set,
};
use utoipa::OpenApi;

mod api;
mod model;
mod revision;
mod service;

/// DDL idempotente. Incluye las tablas del core (la wiki reutiliza su auth, así
/// que necesita `users`; `blackbox_records` lo usa el RecoveryService ante fallos).
const SCHEMA: &[&str] = &[
    "CREATE TABLE IF NOT EXISTS users (
        id UUID PRIMARY KEY,
        username VARCHAR NOT NULL UNIQUE,
        email VARCHAR NOT NULL UNIQUE,
        password_hash VARCHAR NOT NULL DEFAULT '',
        created_at TIMESTAMP NOT NULL
    )",
    "CREATE TABLE IF NOT EXISTS blackbox_records (
        id UUID PRIMARY KEY,
        handler VARCHAR,
        payload JSONB,
        error VARCHAR,
        timestamp TIMESTAMPTZ
    )",
    "CREATE TABLE IF NOT EXISTS pages (
        id UUID PRIMARY KEY,
        slug VARCHAR NOT NULL UNIQUE,
        title VARCHAR NOT NULL,
        content TEXT NOT NULL,
        author VARCHAR NOT NULL,
        created_at TIMESTAMP NOT NULL,
        updated_at TIMESTAMP NOT NULL
    )",
    "CREATE TABLE IF NOT EXISTS page_revisions (
        id UUID PRIMARY KEY,
        page_id UUID NOT NULL,
        revision INTEGER NOT NULL,
        title VARCHAR NOT NULL,
        content TEXT NOT NULL,
        author VARCHAR NOT NULL,
        created_at TIMESTAMP NOT NULL,
        UNIQUE (page_id, revision)
    )",
];

/// Índices de rendimiento (best-effort). Se aplican aparte de las tablas porque
/// `CREATE EXTENSION pg_trgm` puede requerir privilegios que no todo entorno
/// concede: si fallan, la app sigue funcionando (solo más lenta a escala).
///   - idx_pages_updated_at  → ORDER BY updated_at DESC pasa a Index Scan (sin sort).
///   - GIN trigram en lower(content/title) → `LIKE '%q%'` usa índice (búsqueda).
/// (El lookup por `slug` ya está indexado por su UNIQUE.)
const OPTIMIZATIONS: &[&str] = &[
    "CREATE EXTENSION IF NOT EXISTS pg_trgm",
    "CREATE INDEX IF NOT EXISTS idx_pages_updated_at ON pages (updated_at DESC)",
    "CREATE INDEX IF NOT EXISTS idx_pages_content_trgm ON pages USING gin (lower(content) gin_trgm_ops)",
    "CREATE INDEX IF NOT EXISTS idx_pages_title_trgm   ON pages USING gin (lower(title)   gin_trgm_ops)",
    "CREATE INDEX IF NOT EXISTS idx_revisions_page ON page_revisions (page_id, revision DESC)",
];

const SEED_USER: &str = "admin";
const SEED_PASSWORD: &str = "Str0ng-Pass1";

async fn ensure_schema_and_seed(database_url: &str) -> Result<(), Box<dyn std::error::Error>> {
    // Conexión efímera: se cierra al salir del scope, antes de que bootstrap abra
    // su propio pool.
    let db = Database::connect(database_url).await?;

    for stmt in SCHEMA {
        db.execute_unprepared(stmt).await?;
    }
    // Optimizaciones: si alguna falla (p.ej. sin privilegios para CREATE EXTENSION)
    // avisamos y seguimos — la app funciona igual, solo sin ese índice.
    for stmt in OPTIMIZATIONS {
        if let Err(e) = db.execute_unprepared(stmt).await {
            eprintln!("⚠️  optimization skipped: {e}\n    ({stmt})\n    The app works; it may just be slower at scale.");
        }
    }

    // Usuario admin de demo (idempotente). Coste bcrypt bajo a propósito: el
    // build de demo suele ser debug y un coste 12 haría el login lento.
    let exists = core_user::Entity::find()
        .filter(core_user::Column::Username.eq(SEED_USER))
        .one(&db)
        .await?;
    if exists.is_none() {
        let hash = bcrypt::hash(SEED_PASSWORD, 6)?;
        core_user::ActiveModel {
            username: Set(SEED_USER.to_string()),
            email: Set("admin@wiki.local".to_string()),
            password_hash: Set(hash),
            ..Default::default()
        }
        .insert(&db)
        .await?;
        println!("🌱 Seeded demo user '{SEED_USER}' (password: {SEED_PASSWORD})");
    }

    // Página de bienvenida (idempotente).
    let welcome = model::Entity::find()
        .filter(model::Column::Slug.eq("welcome"))
        .one(&db)
        .await?;
    if welcome.is_none() {
        let page = model::ActiveModel {
            slug: Set("welcome".to_string()),
            title: Set("Welcome to Pulse Wiki".to_string()),
            content: Set(WELCOME_CONTENT.to_string()),
            author: Set(SEED_USER.to_string()),
            ..Default::default()
        }
        .insert(&db)
        .await?;
        // Revisión #1 de la página sembrada (coherente con lo que hace el servicio).
        revision::ActiveModel {
            page_id: Set(page.id),
            revision: Set(1),
            title: Set(page.title.clone()),
            content: Set(page.content.clone()),
            author: Set(SEED_USER.to_string()),
            ..Default::default()
        }
        .insert(&db)
        .await?;
        println!("🌱 Seeded 'welcome' page");
    }

    Ok(())
}

const WELCOME_CONTENT: &str = "# Welcome to Pulse Wiki\n\n\
This wiki is a **fully functional** demo app built on top of the *Pulse* framework, \
used as a library crate. ~~It's just a toy~~ — it does real CRUD, auth and history.\n\n\
## What it shows\n\n\
- Custom sea-orm entity (`pages`) + service layer\n\
- JWT auth from the framework: **public reads, authenticated writes**\n\
- The framework's pagination, hybrid cache and error mapping\n\
- Full revision **history** per page (try editing, then *History*)\n\n\
## Markdown support\n\n\
| Feature | Syntax | Works |\n\
| --- | --- | --- |\n\
| Headings | `# .. ######` | yes |\n\
| Emphasis | `**bold** *italic* ~~del~~` | yes |\n\
| Tables | `\\| a \\| b \\|` | yes |\n\
| Images | `![alt](url)` | yes |\n\
| Lists | `-` and `1.` | yes |\n\n\
## Try it\n\n\
1. Click **Login** (user: `admin`, pass: `Str0ng-Pass1`)\n\
2. Hit **New Page**, write some `markdown` and save\n\
3. Edit it, then open **History** to see versions and restore one\n\n\
---\n\n\
> Everything you see is served by a single Rust binary.\n";

#[tokio::main]
async fn main() -> std::io::Result<()> {
    pulse_core::dotenvy::dotenv().ok();

    let database_url = std::env::var("DATABASE_URL")
        .expect("DATABASE_URL must be set (e.g. postgres://wiki:wiki@127.0.0.1:5440/wiki)"); // allow-unwrap

    // El framework exige un JWT_SECRET fuerte; para la demo ponemos uno por
    // defecto si no se ha definido (en producción SIEMPRE debe venir del entorno).
    if std::env::var("JWT_SECRET").is_err() {
        std::env::set_var("JWT_SECRET", "pulse-wiki-demo-secret-key-change-me-please");
    }

    if let Err(e) = ensure_schema_and_seed(&database_url).await {
        eprintln!("❌ Schema/seed failed: {e}");
        std::process::exit(1);
    }

    let config = PulseConfig {
        database_url,
        redis_url: std::env::var("REDIS_URL").ok(),
        host: "0.0.0.0".to_string(),
        port: 8080,
        db_max_connections: 10,
        // La wiki no encola jobs propios; sin handlers el worker queda inactivo.
        handlers: Default::default(),
        // Auth por defecto (BD + JWT). Para AD/LDAP/otra tabla: Some(Arc::new(..)).
        authenticator: None,
        auth_provider: None,
    };

    // Documentación OpenAPI combinada: rutas de la wiki + las del core (login,
    // users, health…), todo en el mismo Swagger UI en /swagger-ui/.
    let mut openapi = api::WikiApiDoc::openapi();
    openapi.merge(pulse_core::api::ApiDoc::openapi());

    // Conexión PROPIA de la wiki: tras desacoplar el ORM, el framework ya no
    // expone su pool por `AppState`, así que la app gestiona la suya y la inyecta
    // en sus rutas. `web::Data` la comparte (Arc) entre los workers de Actix.
    let wiki_db = web::Data::new(
        Database::connect(&config.database_url)
            .await
            .map_err(|e| std::io::Error::other(format!("wiki DB connect failed: {e}")))?,
    );
    let configure = move |cfg: &mut web::ServiceConfig| api::configure(cfg, wiki_db.clone());

    println!("📖 Pulse Wiki on http://localhost:8080  (Swagger: /swagger-ui/)");
    bootstrap(config, configure, openapi).await
}
