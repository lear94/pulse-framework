use crate::core::query::{PageParams, Paginable, PaginatedResult};
use crate::core::transaction::{AtomicFlow, TxResult};
use crate::models::user::{self, Entity as UserEntity};
use crate::pulse::PulseSignal;
use crate::state::AppState;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, Condition, DbErr, EntityTrait, PaginatorTrait, QueryFilter,
    QueryOrder, Set,
};
use std::sync::OnceLock;
use tracing::warn;

pub struct UserService;

/// Hash bcrypt de referencia (coste por defecto) para ejecutar `verify` aun
/// cuando el usuario no existe, igualando la latencia del login y evitando un
/// oráculo de temporización para enumerar usernames. Se calcula una sola vez.
fn dummy_hash() -> &'static str {
    static DUMMY: OnceLock<String> = OnceLock::new();
    DUMMY.get_or_init(|| {
        // Fallback degenerado solo si bcrypt fallara aquí (nunca en la práctica);
        // un hash malformado hace que `verify` devuelva Err -> tratado como no-match.
        bcrypt::hash("pulse-timing-equalizer", bcrypt::DEFAULT_COST)
            .unwrap_or_else(|_| "$invalid$".to_string())
    })
}

impl UserService {
    /// ¿Existe ya un usuario con ese username o email? Para devolver 409 antes
    /// de intentar el INSERT (el índice UNIQUE es el guardián definitivo).
    pub async fn exists(state: &AppState, username: &str, email: &str) -> Result<bool, DbErr> {
        let count = UserEntity::find()
            .filter(
                Condition::any()
                    .add(user::Column::Username.eq(username))
                    .add(user::Column::Email.eq(email)),
            )
            .count(state.db.as_ref())
            .await?;
        Ok(count > 0)
    }

    /// Autentica por username + contraseña. Devuelve el id solo si la
    /// contraseña coincide con el hash almacenado (comparación en tiempo
    /// constante vía bcrypt).
    pub async fn login(state: &AppState, username: String, password: String) -> Option<String> {
        let user = UserEntity::find()
            .filter(user::Column::Username.eq(username))
            .one(state.db.as_ref())
            .await
            .ok()
            .flatten();
        // Siempre ejecutamos bcrypt::verify (contra el hash real o uno dummy) en
        // un hilo bloqueante: es CPU-bound y ahogaría el executor async. Verificar
        // incluso sin usuario iguala la latencia (anti enumeración).
        let (hash, user_id) = match user {
            Some(u) => (u.password_hash, Some(u.id.to_string())),
            None => (dummy_hash().to_string(), None),
        };
        let matched =
            tokio::task::spawn_blocking(move || bcrypt::verify(&password, &hash).unwrap_or(false))
                .await
                .unwrap_or(false);
        match (matched, user_id) {
            (true, Some(id)) => Some(id),
            _ => None,
        }
    }

    pub async fn create_user(
        state: &AppState,
        username: String,
        email: String,
        password: String,
    ) -> TxResult<user::Model> {
        // Hasheamos fuera de la transacción y en un hilo bloqueante: bcrypt es
        // CPU-bound (~decenas de ms) y bloquearía un worker del executor async.
        let password_hash =
            tokio::task::spawn_blocking(move || bcrypt::hash(&password, bcrypt::DEFAULT_COST))
                .await
                .map_err(|e| DbErr::Custom(format!("hash task join failed: {e}")))?
                .map_err(|e| DbErr::Custom(format!("password hashing failed: {e}")))?;

        let execution_result = AtomicFlow::run(state.db.as_ref(), |txn| {
            // [OPT] Movemos los strings dentro del closure (Zero-Copy)
            // Necesitamos clonar para el closure, pero es barato porque son punteros a heap
            let u = username.clone();
            let e = email.clone();
            let ph = password_hash.clone();

            Box::pin(async move {
                let new_user = user::ActiveModel {
                    username: Set(u),
                    email: Set(e),
                    password_hash: Set(ph),
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
                state
                    .pulse
                    .emit(PulseSignal::UserCreated(user.id.to_string()))
                    .await;
                Ok(user)
            }
            Err(e) => Err(e),
        }
    }

    pub async fn find_all(
        state: &AppState,
        params: PageParams,
    ) -> Result<PaginatedResult<user::Model>, sea_orm::DbErr> {
        let paginator = UserEntity::find()
            .order_by_desc(user::Column::CreatedAt)
            .paginate_custom(state.db.as_ref(), &params);
        let total = paginator.num_items().await?;
        let pages = paginator.num_pages().await?;
        let data = paginator.fetch_page(params.page.saturating_sub(1)).await?;
        Ok(PaginatedResult {
            data,
            total,
            page: params.page,
            pages,
        })
    }
}
