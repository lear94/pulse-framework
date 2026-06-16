//! Capa de servicio: toda la lógica de acceso a datos de las páginas.
//!
//! Reutiliza la paginación del framework (`pulse_core::core::query`) para que el
//! listado tenga el mismo formato (`PaginatedResult`) que el resto de la API, y
//! `AtomicFlow` para que cada escritura (página + su revisión) sea transaccional.

use crate::model::{self, ActiveModel, Entity as Page};
use crate::revision;
use pulse_core::core::query::{PageParams, PaginatedResult};
use pulse_core::persistence::seaorm::{AtomicFlow, Paginable};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, DatabaseConnection, DbBackend, DbErr,
    EntityTrait, PaginatorTrait, QueryFilter, QueryOrder, Set, Statement,
};
use uuid::Uuid;

pub struct PageService;

impl PageService {
    pub async fn exists(db: &DatabaseConnection, slug: &str) -> Result<bool, DbErr> {
        let count = Page::find()
            .filter(model::Column::Slug.eq(slug))
            .count(db)
            .await?;
        Ok(count > 0)
    }

    pub async fn get_by_slug(
        db: &DatabaseConnection,
        slug: &str,
    ) -> Result<Option<model::Model>, DbErr> {
        Page::find()
            .filter(model::Column::Slug.eq(slug))
            .one(db)
            .await
    }

    /// Crea la página y su primera revisión en una sola transacción.
    pub async fn create(
        db: &DatabaseConnection,
        slug: String,
        title: String,
        content: String,
        author: String,
    ) -> Result<model::Model, DbErr> {
        AtomicFlow::run(db, move |txn| {
            Box::pin(async move {
                let page = ActiveModel {
                    slug: Set(slug),
                    title: Set(title.clone()),
                    content: Set(content.clone()),
                    author: Set(author.clone()),
                    ..Default::default()
                }
                .insert(txn)
                .await?;

                revision::ActiveModel {
                    page_id: Set(page.id),
                    revision: Set(1),
                    title: Set(title),
                    content: Set(content),
                    author: Set(author),
                    ..Default::default()
                }
                .insert(txn)
                .await?;

                Ok(page)
            })
        })
        .await
    }

    /// Aplica una edición y registra una nueva revisión (atómico). `editor` es
    /// quién hace el cambio; el `author` original de la página no se altera.
    pub async fn update(
        db: &DatabaseConnection,
        slug: &str,
        title: Option<String>,
        content: Option<String>,
        editor: String,
    ) -> Result<Option<model::Model>, DbErr> {
        let Some(existing) = Self::get_by_slug(db, slug).await? else {
            return Ok(None);
        };
        let page_id = existing.id;
        let new_title = title.unwrap_or_else(|| existing.title.clone());
        let new_content = content.unwrap_or_else(|| existing.content.clone());
        let next = Self::next_revision(db, page_id).await?;

        let updated = AtomicFlow::run(db, move |txn| {
            Box::pin(async move {
                let mut active: ActiveModel = existing.into();
                active.title = Set(new_title.clone());
                active.content = Set(new_content.clone());
                let updated = active.update(txn).await?;

                revision::ActiveModel {
                    page_id: Set(page_id),
                    revision: Set(next),
                    title: Set(new_title),
                    content: Set(new_content),
                    author: Set(editor),
                    ..Default::default()
                }
                .insert(txn)
                .await?;

                Ok(updated)
            })
        })
        .await?;

        Ok(Some(updated))
    }

    pub async fn delete(db: &DatabaseConnection, slug: &str) -> Result<bool, DbErr> {
        let res = Page::delete_many()
            .filter(model::Column::Slug.eq(slug))
            .exec(db)
            .await?;
        Ok(res.rows_affected > 0)
    }

    pub async fn list(
        db: &DatabaseConnection,
        params: &PageParams,
    ) -> Result<PaginatedResult<model::Model>, DbErr> {
        // La página usa ORDER BY updated_at DESC → Index Scan (idx_pages_updated_at),
        // sin sort de toda la tabla.
        let data = Page::find()
            .order_by_desc(model::Column::UpdatedAt)
            .paginate_custom(db, params)
            .fetch_page(params.page.saturating_sub(1))
            .await?;
        let total = Self::count_total(db).await?;
        let size = params.size.max(1);
        Ok(PaginatedResult {
            data,
            total,
            page: params.page,
            pages: total.div_ceil(size),
        })
    }

    /// `COUNT(*)` es O(n) y la paginación lo ejecutaría en CADA listado. Para
    /// tablas grandes devolvemos el estimado de `pg_class.reltuples` (O(1),
    /// refrescado por ANALYZE/autovacuum); para tablas pequeñas, COUNT exacto
    /// (barato y fiable). El total pasa a ser aproximado por encima del umbral.
    async fn count_total(db: &DatabaseConnection) -> Result<u64, DbErr> {
        let est = Self::estimated_rows(db).await.unwrap_or(0);
        if est >= 50_000 {
            Ok(est)
        } else {
            Ok(Page::find().count(db).await?)
        }
    }

    async fn estimated_rows(db: &DatabaseConnection) -> Result<u64, DbErr> {
        let stmt = Statement::from_string(
            DbBackend::Postgres,
            "SELECT GREATEST(reltuples, 0)::bigint AS est FROM pg_class WHERE relname = 'pages'",
        );
        let est: i64 = match db.query_one(stmt).await? {
            Some(row) => row.try_get("", "est").unwrap_or(0),
            None => 0,
        };
        Ok(est.max(0) as u64)
    }

    /// Búsqueda por subcadena case-insensitive sobre título y contenido.
    ///
    /// El filtro `lower(col) LIKE '%q%'` usa el índice GIN trigram (pg_trgm). El
    /// CTE `MATERIALIZED` es la clave: obliga a ejecutar el filtro indexado ANTES
    /// del `LIMIT`, evitando que el planner "tome el atajo" de un seq scan que se
    /// para pronto (rápido para términos comunes, pero lentísimo para los raros,
    /// que obligan a escanear media tabla para juntar 50 filas). Así un término
    /// selectivo se resuelve por índice en ~1 ms; el orden por fecha es sobre el
    /// conjunto ya filtrado.
    pub async fn search(db: &DatabaseConnection, q: &str) -> Result<Vec<model::Model>, DbErr> {
        let pattern = format!("%{}%", q.to_lowercase());
        // El LIMIT interno acota cuántos candidatos materializamos: una búsqueda que
        // matchee ≤1000 páginas (todas las realistas) es EXACTA; solo un término
        // presente en casi toda la tabla (caso degenerado) devolvería un subconjunto.
        let stmt = Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"WITH hits AS MATERIALIZED (
                   SELECT * FROM pages
                   WHERE lower(title) LIKE $1 OR lower(content) LIKE $1
                   LIMIT 1000
               )
               SELECT * FROM hits ORDER BY updated_at DESC LIMIT 50"#,
            [pattern.into()],
        );
        Page::find().from_raw_sql(stmt).all(db).await
    }

    // ----- historial de revisiones -----

    async fn next_revision(db: &DatabaseConnection, page_id: Uuid) -> Result<i32, DbErr> {
        let last = revision::Entity::find()
            .filter(revision::Column::PageId.eq(page_id))
            .order_by_desc(revision::Column::Revision)
            .one(db)
            .await?;
        Ok(last.map(|r| r.revision).unwrap_or(0) + 1)
    }

    pub async fn list_revisions(
        db: &DatabaseConnection,
        page_id: Uuid,
    ) -> Result<Vec<revision::Model>, DbErr> {
        revision::Entity::find()
            .filter(revision::Column::PageId.eq(page_id))
            .order_by_desc(revision::Column::Revision)
            .all(db)
            .await
    }

    pub async fn get_revision(
        db: &DatabaseConnection,
        page_id: Uuid,
        rev: i32,
    ) -> Result<Option<revision::Model>, DbErr> {
        revision::Entity::find()
            .filter(revision::Column::PageId.eq(page_id))
            .filter(revision::Column::Revision.eq(rev))
            .one(db)
            .await
    }
}
