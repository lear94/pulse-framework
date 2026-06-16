//! Pulse Chat — chat en tiempo real por WebSocket sobre `pulse_core`.
//!
//! Qué demuestra (y por qué importa tras el desacople del framework):
//!   - Reutiliza SOLO `pulse_core::auth::jwt` (emisión/validación de JWT) — sin
//!     `bootstrap`, sin base de datos, sin Redis. Es la prueba de que el motor
//!     quedó modular: tomas la pieza que necesitas y nada más.
//!   - El handshake del socket se autentica con el MISMO token del framework
//!     (`IdentityProvider::verify_token`): credenciales unificadas HTTP↔WS.
//!   - Fan-out en proceso con `tokio::sync::broadcast` (lock-free, sin actores).
//!     Para multi-nodo, este `Sender` se sustituiría por `pulse_core::pulse`
//!     (PulseReactor sobre Redis) sin tocar la lógica de sesión.
//!
//! Arranque: `cargo run`  →  http://127.0.0.1:8090  (cero infraestructura).

use actix_web::{web, App, HttpRequest, HttpResponse, HttpServer};
use actix_ws::Message;
use futures_util::StreamExt;
use pulse_core::auth::jwt::JwtProvider;
use pulse_core::auth::IdentityProvider;
use serde::Deserialize;
use std::sync::Arc;
use tokio::sync::broadcast;

/// Capacidad del bus de difusión: nº de mensajes en vuelo que un suscriptor lento
/// puede acumular antes de recibir `Lagged` (se le saltan los más viejos, no se
/// bloquea al resto). Potencia de dos: barato para el ring-buffer interno.
const CHANNEL_CAPACITY: usize = 1 << 8; // 256
const BIND_ADDR: (&str, u16) = ("127.0.0.1", 8090);
const DEMO_SECRET: &str = "pulse-chat-demo-secret-change-me-in-prod";
const INDEX_HTML: &str = include_str!("../static/index.html");

/// Estado compartido: el proveedor de identidad del framework + el bus de chat.
struct ChatCtx {
    jwt: Arc<dyn IdentityProvider>,
    bus: broadcast::Sender<String>,
}

#[derive(Deserialize)]
struct LoginReq {
    username: String,
}

#[derive(Deserialize)]
struct WsQuery {
    token: String,
}

/// Empaqueta un evento de chat como JSON para los clientes.
fn event(kind: &str, user: &str, text: &str) -> String {
    serde_json::json!({ "type": kind, "user": user, "text": text }).to_string()
}

async fn index() -> HttpResponse {
    HttpResponse::Ok()
        .content_type("text/html; charset=utf-8")
        .body(INDEX_HTML)
}

/// Demo: emite un JWT del framework para `username` (sin contraseña, es un
/// ejemplo). En una app real esto sería el `/auth/login` del core.
async fn login(ctx: web::Data<ChatCtx>, body: web::Json<LoginReq>) -> HttpResponse {
    let name = body.username.trim();
    if name.is_empty() {
        return HttpResponse::BadRequest().json(serde_json::json!({ "error": "username required" }));
    }
    match ctx.jwt.create_token(name, vec!["user".to_string()]).await {
        Ok(token) => HttpResponse::Ok().json(serde_json::json!({ "token": token })),
        Err(e) => {
            HttpResponse::InternalServerError().json(serde_json::json!({ "error": e.to_string() }))
        }
    }
}

/// Handshake WebSocket. Exige un JWT válido (?token=…) — el mismo que emite el
/// framework — y une al cliente al bus de difusión.
async fn ws(
    req: HttpRequest,
    body: web::Payload,
    ctx: web::Data<ChatCtx>,
    q: web::Query<WsQuery>,
) -> Result<HttpResponse, actix_web::Error> {
    // Autenticación del socket con la identidad del framework.
    let claims = ctx
        .jwt
        .verify_token(&q.token)
        .await
        .map_err(|_| actix_web::error::ErrorUnauthorized("invalid or expired token"))?;
    if !claims.is_access() {
        return Err(actix_web::error::ErrorUnauthorized("not an access token"));
    }
    let user = claims.sub;

    let (response, session, stream) = actix_ws::handle(&req, body)?;
    actix_web::rt::spawn(session_loop(user, session, stream, ctx.bus.clone()));
    Ok(response)
}

/// Bucle de una sesión: multiplexa entradas del socket → bus, y bus → socket.
/// Una sola tarea posee la `Session` (evita sincronizar el escritor del socket).
async fn session_loop(
    user: String,
    mut session: actix_ws::Session,
    mut stream: actix_ws::MessageStream,
    bus: broadcast::Sender<String>,
) {
    let mut rx = bus.subscribe();
    let _ = bus.send(event("system", &user, "joined"));

    loop {
        tokio::select! {
            // Entrante: frame del cliente.
            incoming = stream.next() => match incoming {
                Some(Ok(Message::Text(text))) => {
                    let body = text.trim();
                    if !body.is_empty() {
                        let _ = bus.send(event("chat", &user, body));
                    }
                }
                Some(Ok(Message::Ping(p))) => {
                    if session.pong(&p).await.is_err() { break; }
                }
                // Cierre, error de protocolo o fin de stream → terminamos.
                Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
                _ => {}
            },
            // Saliente: difusión del bus hacia este cliente.
            broadcasted = rx.recv() => match broadcasted {
                Ok(payload) => {
                    if session.text(payload).await.is_err() { break; }
                }
                // Suscriptor lento: nos saltamos lo perdido y seguimos vivos.
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                // El emisor global desapareció (apagado): cerramos.
                Err(broadcast::error::RecvError::Closed) => break,
            },
        }
    }

    let _ = bus.send(event("system", &user, "left"));
    let _ = session.close(None).await;
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    // Solo la pieza de auth del framework. Sin DB, sin bootstrap.
    let jwt: Arc<dyn IdentityProvider> = Arc::new(JwtProvider::new(
        std::env::var("JWT_SECRET").unwrap_or_else(|_| DEMO_SECRET.to_string()),
    ));
    let (bus, _keepalive) = broadcast::channel::<String>(CHANNEL_CAPACITY);
    let ctx = web::Data::new(ChatCtx { jwt, bus });

    println!(
        "💬 Pulse Chat on http://{}:{}  (open two tabs, log in, chat)",
        BIND_ADDR.0, BIND_ADDR.1
    );
    HttpServer::new(move || {
        App::new()
            .app_data(ctx.clone())
            .route("/", web::get().to(index))
            .route("/login", web::post().to(login))
            .route("/ws", web::get().to(ws))
    })
    .bind(BIND_ADDR)?
    .run()
    .await
}
