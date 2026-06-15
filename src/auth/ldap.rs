//! Authenticator contra LDAP / Active Directory (feature `ldap`).
//!
//! Flujo *search + bind*, el correcto para AD:
//!   1. **Bind de servicio** (cuenta de solo lectura) para poder buscar.
//!   2. **Search** del usuario por su login (`sAMAccountName`/`uid`) bajo la base,
//!      recuperando su DN y sus grupos (`memberOf`).
//!   3. **Re-bind como el usuario** con la contraseña recibida → verifica
//!      credenciales contra el directorio (no comparamos hashes nosotros).
//!   4. **Mapeo grupos → roles** de Pulse.
//!
//! Se devuelve `None` ante CUALQUIER fallo (usuario inexistente, password
//! incorrecta, LDAP caído): no se distingue el motivo (anti-enumeración),
//! coherente con el contrato de [`Authenticator`].
//!
//! # Ejemplo
//! ```ignore
//! use pulse_core::auth::ldap::{LdapAuthenticator, LdapConfig};
//! use std::sync::Arc;
//!
//! let ldap = LdapAuthenticator::new(LdapConfig {
//!     url: "ldaps://dc01.corp.local:636".into(),
//!     bind_dn: "CN=svc-pulse,OU=Service,DC=corp,DC=local".into(),
//!     bind_password: std::env::var("LDAP_BIND_PW")?,
//!     user_base_dn: "OU=Users,DC=corp,DC=local".into(),
//!     user_filter: "(sAMAccountName={username})".into(),
//!     group_role_map: vec![
//!         ("Domain Admins".into(), "admin".into()),
//!         ("Pulse-Operators".into(), "operator".into()),
//!     ],
//!     default_roles: vec!["user".into()],
//! });
//! config.authenticator = Some(Arc::new(ldap));
//! ```

use crate::auth::authenticator::{AuthOutcome, Authenticator};
use crate::state::AppState;
use async_trait::async_trait;
use ldap3::{LdapConnAsync, Scope, SearchEntry};
use tracing::warn;

/// Configuración del authenticator LDAP/AD.
pub struct LdapConfig {
    /// URL del servidor: `ldap://host:389` o `ldaps://host:636` (preferible).
    pub url: String,
    /// DN de la cuenta de servicio usada para buscar (solo lectura).
    pub bind_dn: String,
    /// Contraseña de la cuenta de servicio. Pásala desde un secreto, no hardcode.
    pub bind_password: String,
    /// Base bajo la que buscar usuarios, p. ej. `OU=Users,DC=corp,DC=local`.
    pub user_base_dn: String,
    /// Filtro con el placeholder `{username}` (se escapa para evitar inyección).
    /// AD: `(sAMAccountName={username})`; OpenLDAP: `(uid={username})`.
    pub user_filter: String,
    /// Mapeo CN-de-grupo → rol Pulse (comparación case-insensitive).
    pub group_role_map: Vec<(String, String)>,
    /// Roles concedidos a todo usuario autenticado (p. ej. `["user"]`).
    pub default_roles: Vec<String>,
}

pub struct LdapAuthenticator {
    cfg: LdapConfig,
}

impl LdapAuthenticator {
    pub fn new(cfg: LdapConfig) -> Self {
        Self { cfg }
    }

    /// Extrae el CN del primer RDN de un DN (`CN=Admins,OU=...` → `Admins`).
    fn cn_of(dn: &str) -> Option<&str> {
        let (key, value) = dn.split(',').next()?.split_once('=')?;
        if key.trim().eq_ignore_ascii_case("cn") {
            Some(value.trim())
        } else {
            None
        }
    }

    /// Mapea los grupos (`memberOf`, lista de DNs) a roles Pulse + roles por defecto.
    fn map_roles(&self, group_dns: &[String]) -> Vec<String> {
        let mut roles = self.cfg.default_roles.clone();
        for dn in group_dns {
            if let Some(cn) = Self::cn_of(dn) {
                for (group, role) in &self.cfg.group_role_map {
                    if cn.eq_ignore_ascii_case(group) && !roles.contains(role) {
                        roles.push(role.clone());
                    }
                }
            }
        }
        roles
    }

    /// Lógica real; separada para mapear cualquier error a `None` en un solo punto.
    async fn try_authenticate(&self, username: &str, password: &str) -> Result<AuthOutcome, String> {
        // Rechazo explícito de password vacía: muchos servidores LDAP tratan un
        // bind con DN y contraseña vacía como "unauthenticated bind" EXITOSO,
        // lo que permitiría entrar sin credenciales. Cortar aquí es obligatorio.
        if password.is_empty() {
            return Err("empty password".into());
        }

        // 1. Bind de servicio + búsqueda del usuario.
        let (conn, mut ldap) = LdapConnAsync::new(&self.cfg.url)
            .await
            .map_err(|e| e.to_string())?;
        ldap3::drive!(conn);
        ldap.simple_bind(&self.cfg.bind_dn, &self.cfg.bind_password)
            .await
            .map_err(|e| e.to_string())?
            .success()
            .map_err(|e| e.to_string())?;

        // El username se escapa para evitar inyección de filtros LDAP.
        let safe = ldap3::ldap_escape(username);
        let filter = self.cfg.user_filter.replace("{username}", &safe);
        let (entries, _res) = ldap
            .search(
                &self.cfg.user_base_dn,
                Scope::Subtree,
                &filter,
                vec!["memberOf", "sAMAccountName"],
            )
            .await
            .map_err(|e| e.to_string())?
            .success()
            .map_err(|e| e.to_string())?;

        let entry = entries.into_iter().next().ok_or("user not found")?;
        let se = SearchEntry::construct(entry);
        let user_dn = se.dn.clone();
        let _ = ldap.unbind().await;

        // 2. Verificación de credenciales: re-bind como el propio usuario en una
        // conexión nueva. Si la password es incorrecta, el bind falla → None.
        let (conn2, mut ldap2) = LdapConnAsync::new(&self.cfg.url)
            .await
            .map_err(|e| e.to_string())?;
        ldap3::drive!(conn2);
        let bind_res = ldap2
            .simple_bind(&user_dn, password)
            .await
            .map_err(|e| e.to_string())?
            .success();
        let _ = ldap2.unbind().await;
        bind_res.map_err(|e| e.to_string())?;

        // 3. Identidad + roles (memberOf → roles).
        let user_id = se
            .attrs
            .get("sAMAccountName")
            .and_then(|v| v.first())
            .cloned()
            .unwrap_or(user_dn);
        let groups = se.attrs.get("memberOf").cloned().unwrap_or_default();
        Ok(AuthOutcome {
            user_id,
            roles: self.map_roles(&groups),
        })
    }
}

#[async_trait]
impl Authenticator for LdapAuthenticator {
    async fn authenticate(
        &self,
        _state: &AppState,
        username: &str,
        password: &str,
    ) -> Option<AuthOutcome> {
        match self.try_authenticate(username, password).await {
            Ok(outcome) => Some(outcome),
            Err(e) => {
                // Motivo solo a logs (nunca al cliente): anti-enumeración.
                warn!("LDAP auth failed for '{}': {}", username, e);
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn auth(map: &[(&str, &str)]) -> LdapAuthenticator {
        LdapAuthenticator::new(LdapConfig {
            url: String::new(),
            bind_dn: String::new(),
            bind_password: String::new(),
            user_base_dn: String::new(),
            user_filter: "(sAMAccountName={username})".into(),
            group_role_map: map
                .iter()
                .map(|(g, r)| (g.to_string(), r.to_string()))
                .collect(),
            default_roles: vec!["user".into()],
        })
    }

    #[test]
    fn cn_extraction() {
        assert_eq!(
            LdapAuthenticator::cn_of("CN=Domain Admins,OU=Groups,DC=corp,DC=local"),
            Some("Domain Admins")
        );
        assert_eq!(LdapAuthenticator::cn_of("cn=ops,dc=x"), Some("ops"));
        assert_eq!(LdapAuthenticator::cn_of("OU=foo,DC=x"), None);
    }

    #[test]
    fn maps_groups_case_insensitively_with_defaults() {
        let a = auth(&[("Domain Admins", "admin"), ("Pulse-Ops", "operator")]);
        let roles = a.map_roles(&[
            "CN=domain admins,OU=G,DC=corp".into(),
            "CN=Unrelated,OU=G,DC=corp".into(),
        ]);
        assert!(roles.contains(&"user".to_string()), "default role siempre");
        assert!(roles.contains(&"admin".to_string()), "grupo mapeado");
        assert!(!roles.contains(&"operator".to_string()), "grupo no presente");
    }

    #[test]
    fn no_duplicate_roles() {
        let a = auth(&[("g1", "admin"), ("g2", "admin")]);
        let roles = a.map_roles(&["CN=g1,DC=x".into(), "CN=g2,DC=x".into()]);
        assert_eq!(roles.iter().filter(|r| *r == "admin").count(), 1);
    }
}
