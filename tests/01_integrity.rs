#[cfg(test)]
mod integrity_tests {
    use actix_web::{test, web, App};
    use pulse_core::{api, auth::jwt::JwtProvider, state::AppState};
    use pulse_core::store::{memory::MemoryBackend, HybridStore};
    use pulse_core::pulse::memory::MemoryReactor;
    use pulse_core::core::blackbox::{disk::DiskRecorder, FallbackFlightRecorder};
    use pulse_core::core::queue::memory::MemoryQueue;
    use sea_orm::{DatabaseBackend, MockDatabase};
    use std::sync::Arc;

    async fn mock_state() -> AppState {
        let db_conn = MockDatabase::new(DatabaseBackend::Postgres)
            .append_query_results(vec![
                vec![pulse_core::models::user::Model {
                    id: uuid::Uuid::new_v4(),
                    username: "admin".to_owned(),
                    email: "admin@test.com".to_owned(),
                    created_at: chrono::Utc::now().naive_utc(),
                }],
            ])
            .into_connection();

        AppState::new(
            Arc::new(db_conn),
            MemoryReactor::new(100).0,
            HybridStore::new(Arc::new(MemoryBackend)),
            Arc::new(JwtProvider::new("test_secret".into())),
            Arc::new(FallbackFlightRecorder::new(Arc::new(DiskRecorder::new()), Arc::new(DiskRecorder::new()))),
            Arc::new(MemoryQueue::new()),
            None
        )
    }

    #[actix_web::test]
    async fn test_health_check() {
        let state = mock_state().await;
        let app = test::init_service(App::new().app_data(web::Data::new(state)).configure(api::config)).await;
        let req = test::TestRequest::get().uri("/api/v1/health").to_request();
        let resp = test::call_service(&app, req).await;
        assert!(resp.status().is_success());
    }
}
