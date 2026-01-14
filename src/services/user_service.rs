use crate::core::query::{PageParams, Paginable, PaginatedResult};
use crate::core::transaction::{AtomicFlow, TxResult};
use crate::models::user::{self, Entity as UserEntity};
use crate::pulse::PulseSignal;
use crate::state::AppState;
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, QueryOrder, Set};
use tracing::warn;

pub struct UserService;

impl UserService {
    pub async fn login(state: &AppState, username: String) -> Option<String> {
        let user = UserEntity::find()
            .filter(user::Column::Username.eq(username))
            .one(state.db.as_ref())
            .await
            .ok()??;
        Some(user.id.to_string())
    }

    pub async fn create_user(state: &AppState, form: user::Model) -> TxResult<user::Model> {
        // [OPT] Consumir 'form' (Ownership Transfer) para evitar .to_owned()
        let username = form.username; 
        let email = form.email;

        let execution_result = AtomicFlow::run(state.db.as_ref(), |txn| {
            // [OPT] Movemos los strings dentro del closure (Zero-Copy)
            // Necesitamos clonar para el closure, pero es barato porque son punteros a heap
            let u = username.clone(); 
            let e = email.clone();
            
            Box::pin(async move {
                let new_user = user::ActiveModel {
                    username: Set(u),
                    email: Set(e),
                    ..Default::default()
                };
                new_user.insert(txn).await
            })
        })
        .await;

        match execution_result {
            Ok(user) => {
                // Serialización ahora es binaria (bincode) internamente
                if let Err(e) = state.store.set(&user.id.to_string(), &user).await {
                    warn!("Cache update failed: {}", e);
                }
                state.pulse.emit(PulseSignal::UserCreated(user.id.to_string())).await;
                Ok(user)
            }
            Err(e) => Err(e),
        }
    }

    pub async fn find_all(state: &AppState, params: PageParams) -> Result<PaginatedResult<user::Model>, sea_orm::DbErr> {
        let paginator = UserEntity::find()
            .order_by_desc(user::Column::CreatedAt)
            .paginate_custom(state.db.as_ref(), &params);
        let total = paginator.num_items().await?;
        let pages = paginator.num_pages().await?;
        let data = paginator.fetch_page(params.page.saturating_sub(1)).await?;
        Ok(PaginatedResult { data, total, page: params.page, pages })
    }
}
