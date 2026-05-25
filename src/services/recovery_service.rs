use crate::core::blackbox::FlightRecord;
use crate::services::user_service::UserService;
use crate::state::AppState;
use uuid::Uuid;

pub struct RecoveryService;

impl RecoveryService {
    pub async fn capture_failure(
        state: &AppState,
        handler: &str,
        payload: serde_json::Value,
        error: String,
    ) {
        let record = FlightRecord::new(handler, payload, error);
        if let Err(e) = state.blackbox.record(record).await {
            tracing::error!("FATAL: Blackbox system totally failed: {}", e);
        }
    }

    pub async fn list_failures(state: &AppState) -> Vec<FlightRecord> {
        state.blackbox.tail(100).await
    }

    pub async fn replay_from_disk(state: &AppState, target_id: &str) -> Result<String, String> {
        let job = state
            .blackbox
            .scan_id(target_id)
            .await
            .ok_or("Job not found in BlackBox")?;
        match job.handler.as_str() {
            "create_user" => {
                // El payload del blackbox nunca contiene la contraseña (no se
                // persisten credenciales). Resucitamos con una contraseña
                // temporal aleatoria; el usuario deberá restablecerla.
                let username = job.payload["username"]
                    .as_str()
                    .ok_or("Invalid payload: missing username")?
                    .to_string();
                let email = job.payload["email"]
                    .as_str()
                    .ok_or("Invalid payload: missing email")?
                    .to_string();
                let temp_password = Uuid::new_v4().to_string();
                UserService::create_user(state, username, email, temp_password)
                    .await
                    .map(|u| {
                        format!(
                            "REPLAY SUCCESS: User {} resurrected (temporary password set; reset required)",
                            u.id
                        )
                    })
                    .map_err(|e| e.to_string())
            }
            _ => Err(format!("Unknown handler: {}", job.handler)),
        }
    }
}
