//! Helpers acoplados a SeaORM (transacción atómica y paginación tipada).
//!
//! Antes vivían en `core/` pero solo los consume la implementación SeaORM de la
//! persistencia, así que residen aquí: `core` queda libre del ORM.

use crate::core::query::PageParams;
use sea_orm::{
    DatabaseConnection, DatabaseTransaction, DbErr, EntityTrait, PaginatorTrait, Select,
    TransactionError, TransactionTrait,
};
use std::future::Future;
use std::pin::Pin;

pub type TxResult<T> = Result<T, DbErr>;

/// Ejecuta `logic` dentro de una transacción y aplana el doble error
/// (conexión/transacción) de SeaORM en un único `DbErr`.
pub struct AtomicFlow;

impl AtomicFlow {
    pub async fn run<F, T>(db: &DatabaseConnection, logic: F) -> TxResult<T>
    where
        F: for<'c> FnOnce(
                &'c DatabaseTransaction,
            ) -> Pin<Box<dyn Future<Output = TxResult<T>> + Send + 'c>>
            + Send,
        T: Send,
    {
        match db.transaction::<_, T, DbErr>(logic).await {
            Ok(v) => Ok(v),
            Err(TransactionError::Connection(e)) => Err(e),
            Err(TransactionError::Transaction(e)) => Err(e),
        }
    }
}

/// Paginación tipada sobre un `Select<E>` a partir de los `PageParams` del API.
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
