use anyhow::Result;
use clap::{Parser, Subcommand};
use colored::*;
use comfy_table::Table;
use convert_case::{Case, Casing};
use pulse_core::core::blackbox::FlightRecord;
use pulse_core::core::monitor::MonitorSnapshot;
use reqwest;
use std::fs;
use std::io::Write;
use std::path::Path;

#[derive(Parser)]
#[command(name = "pulse")]
#[command(about = "Pulse Framework Ops Tool", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    #[command(name = "new")]
    NewProject { name: String },
    #[command(name = "gen:resource")]
    GenResource { name: String },
    #[command(name = "ops:monitor")]
    OpsMonitor,
    #[command(name = "ops:morgue")]
    OpsMorgue,
    #[command(name = "ops:replay")]
    OpsReplay { id: String },
}

/// Token JWT (rol admin) para los endpoints protegidos. Obtén uno vía
/// `POST /api/v1/auth/login` y expórtalo como PULSE_TOKEN.
fn auth_token() -> Option<String> {
    std::env::var("PULSE_TOKEN").ok().filter(|t| !t.is_empty())
}

#[tokio::main]
async fn main() -> Result<()> {
    let _ = dotenvy::dotenv();
    let api_url = std::env::var("PULSE_API_URL")
        .unwrap_or_else(|_| "http://localhost:8080/api/v1".to_string());
    let cli = Cli::parse();

    match cli.command {
        Commands::OpsMonitor => fetch_monitor(&api_url).await?,
        Commands::OpsMorgue => fetch_morgue(&api_url).await?,
        Commands::OpsReplay { id } => trigger_replay(&api_url, &id).await?,
        Commands::NewProject { name } => create_new_project(&name)?,
        Commands::GenResource { name } => generate_architecture(&name)?,
    }
    Ok(())
}

async fn fetch_monitor(api_url: &str) -> Result<()> {
    println!(
        "{} Connecting to Pulse Uplink at {}...",
        ">>".blue(),
        api_url
    );
    let client = reqwest::Client::new();
    let mut req = client.get(format!("{}/admin/monitor", api_url));
    if let Some(token) = auth_token() {
        req = req.bearer_auth(token);
    }
    let resp = req.send().await?;

    if !resp.status().is_success() {
        println!(
            "{} Failed to fetch monitor: {}",
            ">> ERROR:".red(),
            resp.status()
        );
        return Ok(());
    }

    let m: MonitorSnapshot = resp.json().await?;
    let mut table = Table::new();
    table.set_header(vec!["Metric", "Value"]);
    table.add_row(vec!["Uptime (s)", &m.uptime_seconds.to_string()]);
    table.add_row(vec!["Total Requests", &m.total_requests.to_string()]);
    table.add_row(vec!["Total Failures", &m.total_failures.to_string()]);
    table.add_row(vec!["Active Connections", &m.current_active.to_string()]);
    table.add_row(vec!["RAM Usage (MB)", &m.ram_usage_mb.to_string()]);
    table.add_row(vec!["CPU Usage (%)", &format!("{:.2}", m.cpu_usage)]);
    println!("\n{}", table);
    Ok(())
}

async fn fetch_morgue(api_url: &str) -> Result<()> {
    let client = reqwest::Client::new();
    let mut req = client.get(format!("{}/admin/morgue", api_url));
    if let Some(token) = auth_token() {
        req = req.bearer_auth(token);
    }
    let resp = req.send().await?;
    if !resp.status().is_success() {
        println!(
            "{} Failed to fetch morgue: {}",
            ">> ERROR:".red(),
            resp.status()
        );
        return Ok(());
    }
    let records: Vec<FlightRecord> = resp.json().await?;
    println!(
        "{} Found {} records in blackbox.",
        ">> MORGUE:".yellow().bold(),
        records.len().to_string().cyan()
    );

    if !records.is_empty() {
        let mut table = Table::new();
        table.set_header(vec!["ID", "Handler", "Timestamp", "Error Signature"]);
        for r in records {
            table.add_row(vec![
                r.id.chars().take(8).collect::<String>(),
                r.handler,
                r.timestamp,
                r.error.chars().take(50).collect::<String>(),
            ]);
        }
        println!("{}", table);
        println!("{}", "Use 'pulse ops:replay <id>' to retry a job.".dimmed());
    }
    Ok(())
}

async fn trigger_replay(api_url: &str, id: &str) -> Result<()> {
    let client = reqwest::Client::new();
    let url = format!("{}/admin/replay/{}", api_url, id);
    println!(
        "{} Initiating Lazarus Protocol for ID: {}...",
        ">> REPLAY:".cyan().bold(),
        id.yellow()
    );
    let mut req = client.post(&url);
    if let Some(token) = auth_token() {
        req = req.bearer_auth(token);
    }
    let resp = req.send().await?;
    if resp.status().is_success() {
        println!(
            "{} Replay successful. Job resurrected.",
            ">> SUCCESS:".green().bold()
        );
    } else {
        println!(
            "{} Replay failed: {}",
            ">> ERROR:".red().bold(),
            resp.text().await.unwrap_or_default()
        );
    }
    Ok(())
}

fn create_new_project(name: &str) -> Result<()> {
    let path = Path::new(name);
    if path.exists() {
        return Err(anyhow::anyhow!("Directory '{}' already exists", name));
    }
    println!(
        "{}",
        format!(">> IGNITING NEW PROJECT: {}", name).cyan().bold()
    );

    fs::create_dir_all(format!("{}/src/api", name))?;

    let cargo_toml = format!(
        r#"[package]
name = "{}"
version = "0.1.0"
edition = "2021"

[dependencies]
# El núcleo del framework
pulse_core = {{ path = "../" }}

# Dependencias base sincronizadas con el Core v0.2.1
tokio = {{ version = "1.48", features = ["full", "signal"] }}
serde = {{ version = "1.0.228", features = ["derive"] }}
serde_json = "1.0.145"
utoipa = {{ version = "5.4", features = ["actix_extras", "uuid"] }}
"#,
        name
    );

    write_file(&format!("{}/Cargo.toml", name), &cargo_toml)?;

    let main_rs = r#"use pulse_core::{bootstrap, PulseConfig, actix_web::web, utoipa::OpenApi};

#[derive(OpenApi)]
#[openapi(paths(health))]
struct ApiDoc;

fn routes(cfg: &mut web::ServiceConfig) {
    cfg.route("/health", web::get().to(health));
}

#[utoipa::path(get, path = "/health")]
async fn health() -> impl pulse_core::actix_web::Responder { "System Operational" }

#[tokio::main]
async fn main() -> std::io::Result<()> {
    pulse_core::dotenvy::dotenv().ok();
    let config = PulseConfig {
        database_url: std::env::var("DATABASE_URL").expect("DATABASE_URL required"), // allow-unwrap
        redis_url: std::env::var("REDIS_URL").ok(),
        host: "0.0.0.0".to_string(), port: 8080,
        db_max_connections: 10
    };
    bootstrap(config, routes, ApiDoc::openapi()).await
}
"#;
    write_file(&format!("{}/src/main.rs", name), main_rs)?;

    let env_example =
        "RUST_LOG=info\nDATABASE_URL=postgres://user:pass@localhost:5432/db\n# JWT_SECRET must be >=16 chars; replace with a random value (openssl rand -base64 48)\nJWT_SECRET=change-me-to-a-long-random-secret\nPULSE_ADMIN_USERS=\n";
    write_file(&format!("{}/.env", name), env_example)?;

    println!(
        "{} Project ready. Run 'cd {} && cargo run'",
        ">> DONE:".green(),
        name
    );
    Ok(())
}

fn generate_architecture(name: &str) -> Result<()> {
    let snake = name.to_case(Case::Snake);
    let pascal = name.to_case(Case::Pascal);
    println!(">> Generating resource: {}", pascal);

    let api_code = format!(
        r#"use pulse_core::actix_web::{{web, HttpResponse, Responder}};
use utoipa;

pub fn configure(cfg: &mut web::ServiceConfig) {{
    cfg.route("/{}", web::get().to(list));
}}

#[utoipa::path(get, path = "/api/v1/{}")]
async fn list() -> impl Responder {{
    HttpResponse::Ok().json(serde_json::json!([{{ "name": "Test Resource" }}]))
}}
"#,
        snake, snake
    );

    write_file(&format!("src/api/{}.rs", snake), &api_code)?;
    println!(">> Created src/api/{}.rs", snake);
    Ok(())
}

fn write_file(path: &str, content: &str) -> Result<()> {
    if let Some(parent) = Path::new(path).parent() {
        fs::create_dir_all(parent)?;
    }
    let mut file = fs::File::create(path)?;
    file.write_all(content.as_bytes())?;
    Ok(())
}
