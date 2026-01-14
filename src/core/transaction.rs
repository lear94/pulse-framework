use sea_orm::{DatabaseConnection, DatabaseTransaction, DbErr, TransactionError, TransactionTrait};
use std::future::Future;
use std::pin::Pin;

pub type TxResult<T> = Result<T, DbErr>;

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
