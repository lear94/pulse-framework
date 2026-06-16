#[cfg(test)]
mod integrity_tests {
    use actix_web::{test, web, App};
    use pulse_core::auth::revocation::MemoryRevocationStore;
    use pulse_core::core::blackbox::{disk::DiskRecorder, FallbackFlightRecorder};
    use pulse_core::auth::authenticator::DbAuthenticator;
    use pulse_core::core::queue::memory::MemoryQueue;
    use pulse_core::core::ratelimit::MemoryRateLimiter;
    use pulse_core::persistence::seaorm::{SeaOrmDatastore, SeaOrmUserRepository};
    use pulse_core::pulse::memory::MemoryReactor;
    use pulse_core::store::{memory::MemoryBackend, HybridStore};
    use pulse_core::{api, auth::jwt::JwtProvider, state::AppState};
    use sea_orm::{DatabaseBackend, MockDatabase};
    use std::sync::Arc;
    use std::time::Duration;

    // Límite de rate bajo para poder ejercitarlo en los tests.
    const TEST_RATE_MAX: u32 = 5;

    async fn mock_state() -> AppState {
        let db_conn = MockDatabase::new(DatabaseBackend::Postgres)
            .append_query_results(vec![vec![pulse_core::models::user::Model {
                id: uuid::Uuid::new_v4(),
                username: "admin".to_owned(),
                email: "admin@test.com".to_owned(),
                password_hash: String::new(),
                created_at: chrono::Utc::now().naive_utc(),
            }]])
            .into_connection();
        let db = Arc::new(db_conn);

        AppState::new(
            Arc::new(SeaOrmUserRepository::new(db.clone())),
            Arc::new(SeaOrmDatastore::new(db.clone())),
            MemoryReactor::new(100).0,
            HybridStore::new(Arc::new(MemoryBackend)),
            Arc::new(JwtProvider::new("test_secret_value_long".into())),
            Arc::new(DbAuthenticator),
            Arc::new(FallbackFlightRecorder::new(
                Arc::new(DiskRecorder::new()),
                Arc::new(DiskRecorder::new()),
            )),
            Arc::new(MemoryQueue::new()),
            None,
            Arc::new(MemoryRevocationStore::new()),
            Arc::new(MemoryRateLimiter::new(TEST_RATE_MAX, Duration::from_secs(60))),
        )
    }

    #[actix_web::test]
    async fn test_health_check() {
        let state = mock_state().await;
        let app = test::init_service(
            App::new()
                .app_data(web::Data::new(state))
                .configure(api::config),
        )
        .await;
        let req = test::TestRequest::get().uri("/api/v1/health").to_request();
        let resp = test::call_service(&app, req).await;
        assert!(resp.status().is_success());
    }

    #[actix_web::test]
    async fn test_metrics_endpoint() {
        let state = mock_state().await;
        let app = test::init_service(
            App::new()
                .app_data(web::Data::new(state))
                .configure(api::config),
        )
        .await;
        let req = test::TestRequest::get().uri("/api/v1/metrics").to_request();
        let resp = test::call_service(&app, req).await;
        assert!(resp.status().is_success());
        let body = test::read_body(resp).await;
        let text = String::from_utf8_lossy(&body);
        assert!(text.contains("pulse_requests_total"));
    }

    #[actix_web::test]
    async fn test_protected_route_requires_auth() {
        let state = mock_state().await;
        let app = test::init_service(
            App::new()
                .app_data(web::Data::new(state))
                .configure(api::config),
        )
        .await;
        // /users (GET) exige Claims → sin token debe ser 401.
        let req = test::TestRequest::get().uri("/api/v1/users").to_request();
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status().as_u16(), 401);
    }

    #[actix_web::test]
    async fn test_admin_route_requires_auth() {
        let state = mock_state().await;
        let app = test::init_service(
            App::new()
                .app_data(web::Data::new(state))
                .configure(api::config),
        )
        .await;
        let req = test::TestRequest::get()
            .uri("/api/v1/admin/monitor")
            .to_request();
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status().as_u16(), 401);
    }

    #[actix_web::test]
    async fn test_login_rate_limited() {
        let state = mock_state().await;
        let app = test::init_service(
            App::new()
                .app_data(web::Data::new(state))
                .configure(api::config),
        )
        .await;

        let mut saw_429 = false;
        for _ in 0..(TEST_RATE_MAX + 2) {
            let req = test::TestRequest::post()
                .uri("/api/v1/auth/login")
                .set_json(serde_json::json!({ "username": "x", "password": "y" }))
                .to_request();
            let resp = test::call_service(&app, req).await;
            if resp.status().as_u16() == 429 {
                saw_429 = true;
                break;
            }
        }
        assert!(
            saw_429,
            "expected a 429 after exceeding the login rate limit"
        );
    }
}
