//! Verificación de credenciales enchufable (el "método de login").
//!
//! El framework separa DOS responsabilidades de auth:
//!   1. **Verificar credenciales** → `Authenticator` (este módulo). Decide SI el
//!      usuario es quien dice y QUÉ roles tiene. Aquí se enchufa AD/LDAP, otra
//!      tabla, un IdP externo, etc.
//!   2. **Emitir/validar el token** → [`crate::auth::IdentityProvider`] (JWT por
//!      defecto). Tras un `authenticate` exitoso, Pulse emite SU token.
//!
//! Por defecto se usa [`DbAuthenticator`] (tabla `users` + bcrypt). Para usar
//! otra fuente, implementa `Authenticator` y pásalo en `PulseConfig::authenticator`.
//!
//! # Ejemplo: autenticar contra Active Directory (LDAP)
//! ```ignore
//! use pulse_core::auth::authenticator::{Authenticator, AuthOutcome};
//! use pulse_core::state::AppState;
//! use pulse_core::async_trait::async_trait;
//!
//! struct AdAuthenticator { ldap_url: String, base_dn: String }
//!
//! #[async_trait]
//! impl Authenticator for AdAuthenticator {
//!     async fn authenticate(&self, _state: &AppState, username: &str, password: &str)
//!         -> Option<AuthOutcome>
//!     {
//!         // 1. BIND contra AD con (username, password). Si falla -> None.
//!         // 2. Leer grupos del usuario y mapearlos a roles de Pulse.
//!         let bound = ldap_bind(&self.ldap_url, &self.base_dn, username, password).await.ok()?;
//!         let roles = map_ad_groups_to_roles(&bound.groups);
//!         Some(AuthOutcome { user_id: bound.sid, roles })
//!     }
//! }
//! // En main: config.authenticator = Some(Arc::new(AdAuthenticator { .. }));
//! ```

use crate::api::resolve_roles;
use crate::services::user_service::UserService;
use crate::state::AppState;
use async_trait::async_trait;

/// Resultado de una autenticación correcta: a quién pertenece la sesión y con
/// qué roles. `roles` viaja dentro del JWT emitido a continuación.
pub struct AuthOutcome {
    pub user_id: String,
    pub roles: Vec<String>,
}

#[async_trait]
pub trait Authenticator: Send + Sync {
    /// Verifica `username`/`password` contra la fuente de identidad.
    ///
    /// Devuelve `None` ante CUALQUIER fallo (usuario inexistente, password
    /// incorrecta, backend caído): no se distingue el motivo, para no filtrar
    /// si un usuario existe (anti-enumeración). Mantén la latencia uniforme
    /// entre los caminos de éxito y fallo cuando sea posible.
    async fn authenticate(
        &self,
        state: &AppState,
        username: &str,
        password: &str,
    ) -> Option<AuthOutcome>;
}

/// Implementación por defecto: tabla `users` (bcrypt) + roles vía allowlist
/// `PULSE_ADMIN_USERS`. Es la que usa `bootstrap` si la app no inyecta otra.
pub struct DbAuthenticator;

#[async_trait]
impl Authenticator for DbAuthenticator {
    async fn authenticate(
        &self,
        state: &AppState,
        username: &str,
        password: &str,
    ) -> Option<AuthOutcome> {
        let user_id =
            UserService::login(state, username.to_string(), password.to_string()).await?;
        Some(AuthOutcome {
            user_id,
            roles: resolve_roles(username),
        })
    }
}
