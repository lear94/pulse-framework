use crate::core::query::{PageParams, PaginatedResult};
use crate::persistence::{NewUser, RepoError, RepoResult, User};
use crate::pulse::PulseSignal;
use crate::state::AppState;
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
    pub async fn exists(state: &AppState, username: &str, email: &str) -> RepoResult<bool> {
        state.users.exists(username, email).await
    }

    /// Autentica por username + contraseña. Devuelve el id solo si la
    /// contraseña coincide con el hash almacenado (comparación en tiempo
    /// constante vía bcrypt).
    pub async fn login(state: &AppState, username: String, password: String) -> Option<String> {
        let user = state.users.find_by_username(&username).await.ok().flatten();
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
    ) -> RepoResult<User> {
        // Hasheamos fuera de la persistencia y en un hilo bloqueante: bcrypt es
        // CPU-bound (~decenas de ms) y bloquearía un worker del executor async.
        // El hashing es lógica de dominio, por eso vive aquí y no en el repo.
        let password_hash =
            tokio::task::spawn_blocking(move || bcrypt::hash(&password, bcrypt::DEFAULT_COST))
                .await
                .map_err(|e| RepoError::Backend(format!("hash task join failed: {e}")))?
                .map_err(|e| RepoError::Backend(format!("password hashing failed: {e}")))?;

        let user = state
            .users
            .insert(NewUser {
                username,
                email,
                password_hash,
            })
            .await?;

        // Serialización binaria (bincode) internamente.
        if let Err(e) = state.store.set(&user.id.to_string(), &user).await {
            warn!("Cache update failed: {}", e);
        }
        state
            .pulse
            .emit(PulseSignal::UserCreated(user.id.to_string()))
            .await;
        Ok(user)
    }

    pub async fn find_all(
        state: &AppState,
        params: PageParams,
    ) -> RepoResult<PaginatedResult<User>> {
        state.users.find_all(&params).await
    }
}
