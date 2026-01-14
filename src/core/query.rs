use sea_orm::{DatabaseConnection, EntityTrait, PaginatorTrait, Select};
use serde::Deserialize;
use utoipa::IntoParams;

#[derive(Debug, Deserialize, IntoParams)]
pub struct PageParams {
    #[serde(default = "default_page")]
    pub page: u64,
    #[serde(default = "default_size")]
    pub size: u64,
}

fn default_page() -> u64 {
    1
}
fn default_size() -> u64 {
    10
}

#[derive(serde::Serialize, utoipa::ToSchema)]
pub struct PaginatedResult<T> {
    pub data: Vec<T>,
    pub total: u64,
    pub page: u64,
    pub pages: u64,
}

pub trait Paginable<E: EntityTrait>
where
    E::Model: Sync,
{
    fn paginate_custom<'db>(
        self,
        db: &'db DatabaseConnection,
        params: &PageParams,
    ) -> sea_orm::Paginator<'db, DatabaseConnection, sea_orm::SelectModel<E::Model>>;
}

impl<E: EntityTrait> Paginable<E> for Select<E>
where
    E::Model: Sync,
{
    fn paginate_custom<'db>(
        self,
        db: &'db DatabaseConnection,
        params: &PageParams,
    ) -> sea_orm::Paginator<'db, DatabaseConnection, sea_orm::SelectModel<E::Model>> {
        PaginatorTrait::paginate(self, db, params.size)
    }
}
