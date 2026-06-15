use migration::Migrator;
use sea_orm_migration::prelude::*;
use std::io::{IsTerminal, Write};

/// Comandos que ELIMINAN tablas en masa — los equivalentes peligrosos al
/// `migrate:fresh` de Laravel. Quedan tras un candado explícito:
///   - `fresh`   → DROP de TODAS las tablas del esquema, luego reaplica todo.
///   - `refresh` → rollback de TODAS las migraciones (drop), luego reaplica.
///   - `reset`   → rollback de TODAS las migraciones (drop).
const DESTRUCTIVE: &[&str] = &["fresh", "refresh", "reset"];

/// Candado: variable de entorno con valor literal EXACTO, para que no se active
/// por accidente (un `=1` despistado no basta).
const UNLOCK_VAR: &str = "PULSE_ALLOW_DESTRUCTIVE_MIGRATIONS";
const UNLOCK_VALUE: &str = "yes-i-understand-this-drops-tables";

#[tokio::main]
async fn main() {
    let cmd = detect_subcommand();

    if DESTRUCTIVE.contains(&cmd.as_str()) && !destructive_allowed(&cmd) {
        std::process::exit(1);
    }
    // `down` es rollback controlado (N migraciones), no un borrado masivo, pero
    // ejecuta los `down()` que pueden dropear tablas: lo avisamos sin bloquear.
    if cmd == "down" {
        eprintln!("⚠️  `migration down` rolls back migrations and may drop tables. Proceeding.");
    }

    cli::run_cli(Migrator).await;
}

/// Detecta el subcomando = primer argumento posicional, saltando las opciones
/// globales de la CLI de sea-orm-migration (`-u/--database-url <v>`,
/// `-s/--database-schema <v>` consumen valor; `-v` es booleana).
fn detect_subcommand() -> String {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        if matches!(a.as_str(), "-u" | "--database-url" | "-s" | "--database-schema") {
            i += 2; // opción con valor: salta flag + valor
            continue;
        }
        if a.starts_with('-') {
            i += 1; // flag booleana o `--flag=valor`
            continue;
        }
        return a.to_lowercase(); // primer posicional = subcomando
    }
    String::new()
}

/// Imprime el motivo del bloqueo y aplica el doble candado:
///   1. La env var debe valer EXACTAMENTE `UNLOCK_VALUE`.
///   2. Si hay terminal interactiva, además se exige teclear el comando.
/// Devuelve `true` solo si ambos pasan.
fn destructive_allowed(cmd: &str) -> bool {
    eprintln!();
    eprintln!("⛔ BLOCKED: `migration {cmd}` is DESTRUCTIVE and drops tables.");
    match cmd {
        "fresh" => {
            eprintln!("   `fresh` DROPS ALL TABLES in the schema, then re-runs every migration.")
        }
        "refresh" => eprintln!(
            "   `refresh` rolls back ALL migrations (dropping their tables), then re-runs them."
        ),
        "reset" => eprintln!("   `reset` rolls back ALL migrations, dropping their tables."),
        _ => {}
    }

    // Candado 1: env var con valor literal exacto.
    let unlocked = std::env::var(UNLOCK_VAR).map(|v| v == UNLOCK_VALUE).unwrap_or(false);
    if !unlocked {
        eprintln!();
        eprintln!("   Locked to prevent accidental data loss. To proceed intentionally, set:");
        eprintln!("       {UNLOCK_VAR}={UNLOCK_VALUE}");
        eprintln!("   (Forward-only is the norm: prefer `up`. Use `down -n N` to roll back N steps.)");
        eprintln!();
        return false;
    }

    // Candado 2 (solo con TTY): confirmación manual tecleando el comando.
    if std::io::stdin().is_terminal() {
        eprint!("   {UNLOCK_VAR} is set. Type `{cmd}` to confirm: ");
        let _ = std::io::stderr().flush();
        let mut line = String::new();
        if std::io::stdin().read_line(&mut line).is_err() || line.trim() != cmd {
            eprintln!("   Confirmation failed; aborting.");
            return false;
        }
    }

    eprintln!("   ⚠️  Proceeding with destructive `{cmd}` (explicitly unlocked).");
    true
}
