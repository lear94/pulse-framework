use gloo_timers::future::TimeoutFuture;
use leptos::*;
use serde::{Deserialize, Serialize};

const API_URL: &str = "http://localhost:8080/api/v1";
// Token JWT (rol admin) embebido en build: `PULSE_TOKEN=... trunk build`.
// Los endpoints /admin/* exigen rol admin.
const AUTH_TOKEN: Option<&str> = option_env!("PULSE_TOKEN");

fn admin_get(client: &reqwest::Client, url: String) -> reqwest::RequestBuilder {
    let mut req = client.get(url);
    if let Some(token) = AUTH_TOKEN {
        req = req.bearer_auth(token);
    }
    req
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct MonitorSnapshot {
    uptime_seconds: u64,
    total_requests: usize,
    total_failures: usize,
    ram_usage_mb: u64,
    cpu_usage: f32,
    current_active: usize,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
struct FlightRecord {
    id: String,
    handler: String,
    error: String,
    timestamp: String,
}

#[derive(Clone, Debug, PartialEq)]
enum SystemStatus {
    Idle,
    Processing(String),
    Success(String),
    Error(String),
}

#[component]
fn App() -> impl IntoView {
    let (monitor, set_monitor) = create_signal(None::<MonitorSnapshot>);
    let (morgue, set_morgue) = create_signal(Vec::<FlightRecord>::new());
    let (status, set_status) = create_signal(SystemStatus::Idle);

    create_effect(move |_| {
        spawn_local(async move {
            let client = reqwest::Client::new();
            loop {
                if let Ok(res) = admin_get(&client, format!("{}/admin/monitor", API_URL))
                    .send()
                    .await
                {
                    if let Ok(data) = res.json::<MonitorSnapshot>().await {
                        set_monitor.set(Some(data));
                    }
                }
                if let Ok(res) = admin_get(&client, format!("{}/admin/morgue", API_URL))
                    .send()
                    .await
                {
                    if let Ok(data) = res.json::<Vec<FlightRecord>>().await {
                        set_morgue.update(|current| {
                            if current.len() != data.len() {
                                *current = data;
                            }
                        });
                    }
                }
                TimeoutFuture::new(2000).await;
            }
        });
    });

    let trigger_replay = move |id: String| {
        set_status.set(SystemStatus::Processing(format!(
            "INITIATING LAZARUS PROTOCOL: {}...",
            &id[..8]
        )));
        spawn_local(async move {
            let client = reqwest::Client::new();
            let mut replay_req = client.post(format!("{}/admin/replay/{}", API_URL, id));
            if let Some(token) = AUTH_TOKEN {
                replay_req = replay_req.bearer_auth(token);
            }
            match replay_req.send().await {
                Ok(resp) => {
                    if resp.status().is_success() {
                        set_status.set(SystemStatus::Success(format!(
                            "REPLAY SUCCESSFUL: {}",
                            &id[..8]
                        )));
                        set_morgue.update(|records| {
                            records.retain(|r| r.id != id);
                        });
                        TimeoutFuture::new(3000).await;
                        set_status.set(SystemStatus::Idle);
                    } else {
                        set_status.set(SystemStatus::Error(format!(
                            "SERVER DENIED REPLAY: {}",
                            resp.status()
                        )));
                    }
                }
                Err(e) => set_status.set(SystemStatus::Error(format!("NETWORK FAILURE: {}", e))),
            }
        });
    };

    view! {
        <div class="container">
            <header>
                <div style="display: flex; justify-content: space-between; align-items: center;">
                    <h1>PULSE <span style="font-size: 0.5em; vertical-align: super;">v3.0</span></h1>
                    <div class="status-badge live">SYSTEM ONLINE</div>
                </div>
                <div style="height: 20px; font-size: 0.9em;">
                    {move || match status.get() {
                        SystemStatus::Idle => view! { <span style="color: #64748b;">WAITING FOR COMMAND...</span> }.into_view(),
                        SystemStatus::Processing(msg) => view! { <span style="color: #facc15;">{msg}</span> }.into_view(),
                        SystemStatus::Success(msg) => view! { <span style="color: #4ade80;">{msg}</span> }.into_view(),
                        SystemStatus::Error(msg) => view! { <span style="color: #f87171;">{msg}</span> }.into_view(),
                    }}
                </div>
            </header>
            <main>
                <h2>System Telemetry (HUD)</h2>
                {move || match monitor.get() {
                    Some(m) => view! {
                        <div class="hud">
                            <div class="card"><div class="card-title">Uptime</div><div class="card-value">{format!("{}s", m.uptime_seconds)}</div></div>
                            <div class="card"><div class="card-title">Requests</div><div class="card-value green">{m.total_requests}</div></div>
                            <div class="card"><div class="card-title">Failures</div><div class="card-value red">{m.total_failures}</div></div>
                            <div class="card"><div class="card-title">RAM (MB)</div><div class="card-value">{m.ram_usage_mb}</div></div>
                        </div>
                    }.into_view(),
                    None => view! { <div style="padding: 50px; text-align: center; color: #64748b;">ESTABLISHING UPLINK...</div> }.into_view()
                }}
                <h2>Blackbox Retention (Morgue)</h2>
                <div style="background: #1e293b; border: 1px solid #334155; border-radius: 8px; overflow: hidden;">
                    <table>
                        <thead><tr><th width="120">ID</th><th width="200">Handler</th><th>Error Signature</th><th width="100">Action</th></tr></thead>
                        <tbody>
                            {move || if morgue.get().is_empty() {
                                // CORRECCIÓN AQUÍ: Comillas añadidas al texto
                                view! { <tr><td colspan="4" style="text-align: center; padding: 30px; color: #64748b;">"// NO CASUALTIES DETECTED //"</td></tr> }.into_view()
                            } else {
                                morgue.get().into_iter().map(|job| {
                                    let id_clone = job.id.clone();
                                    view! {
                                        <tr>
                                            <td style="font-family: monospace;">{job.id.chars().take(8).collect::<String>()}</td>
                                            <td style="color: #7dd3fc;">{job.handler}</td>
                                            <td style="color: #f87171;">{job.error.chars().take(60).collect::<String>()}...</td>
                                            <td><button class="btn-replay" on:click=move |_| trigger_replay(id_clone.clone())>REPLAY</button></td>
                                        </tr>
                                    }
                                }).collect_view()
                            }}
                        </tbody>
                    </table>
                </div>
            </main>
        </div>
    }
}

fn main() {
    console_error_panic_hook::set_once();
    mount_to_body(|| view! { <App/> })
}
