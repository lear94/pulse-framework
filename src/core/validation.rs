//! Validaciones de input sin dependencias externas. Devuelven `Err(mensaje)`
//! con un texto apto para responder al cliente (400 Bad Request).

pub type ValidationResult = Result<(), String>;

/// Validación pragmática de email: una sola `@`, con parte local y dominio no
/// vacíos, y al menos un punto en el dominio. No pretende cubrir el RFC 5322
/// completo (imposible con un regex sano), solo descartar entradas obviamente
/// inválidas.
pub fn validate_email(email: &str) -> ValidationResult {
    let email = email.trim();
    if email.is_empty() || email.len() > 254 {
        return Err("email must be between 1 and 254 characters".into());
    }
    let parts: Vec<&str> = email.split('@').collect();
    if parts.len() != 2 {
        return Err("email must contain exactly one '@'".into());
    }
    let (local, domain) = (parts[0], parts[1]);
    if local.is_empty() {
        return Err("email local part is empty".into());
    }
    if domain.is_empty()
        || !domain.contains('.')
        || domain.starts_with('.')
        || domain.ends_with('.')
    {
        return Err("email domain is invalid".into());
    }
    if email.contains(char::is_whitespace) {
        return Err("email must not contain whitespace".into());
    }
    Ok(())
}

/// Reglas de username: 3..=32 chars, solo alfanumérico, `_`, `-` o `.`.
pub fn validate_username(username: &str) -> ValidationResult {
    let len = username.chars().count();
    if !(3..=32).contains(&len) {
        return Err("username must be between 3 and 32 characters".into());
    }
    if !username
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
    {
        return Err("username may only contain letters, digits, '_', '-' or '.'".into());
    }
    Ok(())
}

/// Política de contraseñas: mínimo 10 chars, máximo 128 (límite de bcrypt: 72
/// bytes significativos, pero validamos longitud razonable), y al menos tres de
/// cuatro clases (minúscula, mayúscula, dígito, símbolo).
pub fn validate_password(password: &str) -> ValidationResult {
    let len = password.chars().count();
    if len < 10 {
        return Err("password must be at least 10 characters".into());
    }
    if password.len() > 128 {
        return Err("password must be at most 128 bytes".into());
    }
    let mut classes = 0;
    if password.chars().any(|c| c.is_ascii_lowercase()) {
        classes += 1;
    }
    if password.chars().any(|c| c.is_ascii_uppercase()) {
        classes += 1;
    }
    if password.chars().any(|c| c.is_ascii_digit()) {
        classes += 1;
    }
    if password.chars().any(|c| !c.is_ascii_alphanumeric()) {
        classes += 1;
    }
    if classes < 3 {
        return Err(
            "password must include at least 3 of: lowercase, uppercase, digit, symbol".into(),
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emails() {
        assert!(validate_email("a@b.com").is_ok());
        assert!(validate_email("bad").is_err());
        assert!(validate_email("a@@b.com").is_err());
        assert!(validate_email("a@b").is_err());
        assert!(validate_email("a b@c.com").is_err());
    }

    #[test]
    fn usernames() {
        assert!(validate_username("engineer_one").is_ok());
        assert!(validate_username("ab").is_err());
        assert!(validate_username("bad name").is_err());
    }

    #[test]
    fn passwords() {
        assert!(validate_password("Str0ng-pass").is_ok());
        assert!(validate_password("short1A").is_err());
        assert!(validate_password("alllowercaseonly").is_err());
    }
}
