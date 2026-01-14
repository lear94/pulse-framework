use crate::core::blackbox::FlightRecord;
use crate::models::user;
use crate::services::user_service::UserService;
use crate::state::AppState;

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
                let form: user::Model = serde_json::from_value(job.payload.clone())
                    .map_err(|_| "Invalid payload structure")?;
                UserService::create_user(state, form)
                    .await
                    .map(|u| format!("REPLAY SUCCESS: User {} resurrected", u.id))
                    .map_err(|e| e.to_string())
            }
            _ => Err(format!("Unknown handler: {}", job.handler)),
        }
    }
}
