//! End-to-end tests against a mock Garmin server. No real network access.

use std::time::Duration;

use base64::Engine;
use garmin_connect::{
    Endpoints, Error, GarminClient, LoginOptions, MemoryTokenStore, MfaMethod, TokenStore, Tokens,
};
use serde_json::json;
use wiremock::matchers::{body_partial_json, body_string_contains, header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn jwt(exp: i64, client_id: &str) -> String {
    let e = base64::engine::general_purpose::URL_SAFE_NO_PAD;
    format!(
        "{}.{}.sig",
        e.encode(br#"{"alg":"RS256"}"#),
        e.encode(json!({"exp": exp, "client_id": client_id}).to_string())
    )
}

fn fixture(name: &str) -> serde_json::Value {
    let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn fast() -> LoginOptions {
    LoginOptions { submit_delay: Duration::ZERO }
}

fn far_future() -> i64 {
    chrono::Utc::now().timestamp() + 3600 * 24
}

async fn mock_signin_page(server: &MockServer) {
    Mock::given(method("GET"))
        .and(path("/mobile/sso/en_US/sign-in"))
        .and(query_param("clientId", "GCM_IOS_DARK"))
        .respond_with(ResponseTemplate::new(200).insert_header("set-cookie", "SESSION=abc; Path=/"))
        .expect(1)
        .mount(server)
        .await;
}

async fn mock_token_exchange(server: &MockServer, access: &str) {
    Mock::given(method("POST"))
        .and(path("/di-oauth2-service/oauth/token"))
        .and(body_string_contains("service_ticket=ST-12345"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"access_token": access, "refresh_token": "refresh-1"})),
        )
        .expect(1)
        .mount(server)
        .await;
}

async fn mock_profile(server: &MockServer) {
    Mock::given(method("GET"))
        .and(path("/userprofile-service/socialProfile"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"displayName": "runner 42"})))
        .mount(server)
        .await;
}

#[tokio::test]
async fn login_without_mfa_saves_tokens() {
    let server = MockServer::start().await;
    mock_signin_page(&server).await;
    Mock::given(method("POST"))
        .and(path("/mobile/api/login"))
        .and(header("cookie", "SESSION=abc"))
        .and(body_partial_json(json!({"username": "me@example.com", "password": "pw"})))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "responseStatus": {"type": "SUCCESSFUL"},
            "serviceTicketId": "ST-12345"
        })))
        .expect(1)
        .mount(&server)
        .await;
    let access = jwt(far_future(), "GARMIN_CONNECT_MOBILE_ANDROID_DI_2025Q2");
    mock_token_exchange(&server, &access).await;
    mock_profile(&server).await;

    let client =
        GarminClient::with_endpoints(Endpoints::single_host(&server.uri()), MemoryTokenStore::new()).unwrap();
    client
        .login_with_options("me@example.com", "pw", &fast(), |_| panic!("MFA should not be asked for"))
        .await
        .unwrap();

    assert_eq!(client.display_name().await.unwrap(), "runner 42");
    assert!(client.has_tokens().await.unwrap());
}

#[tokio::test]
async fn login_with_mfa_asks_for_code() {
    let server = MockServer::start().await;
    mock_signin_page(&server).await;
    Mock::given(method("POST"))
        .and(path("/mobile/api/login"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "responseStatus": {"type": "MFA_REQUIRED"},
            "customerMfaInfo": {"mfaLastMethodUsed": "email"}
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/mobile/api/mfa/verifyCode"))
        .and(body_partial_json(json!({"mfaMethod": "email", "mfaVerificationCode": "123456"})))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "responseStatus": {"type": "SUCCESSFUL"},
            "serviceTicketId": "ST-12345"
        })))
        .expect(1)
        .mount(&server)
        .await;
    mock_token_exchange(&server, &jwt(far_future(), "CID")).await;
    mock_profile(&server).await;

    let client =
        GarminClient::with_endpoints(Endpoints::single_host(&server.uri()), MemoryTokenStore::new()).unwrap();
    let mut asked = None;
    client
        .login_with_options("me@example.com", "pw", &fast(), |m| {
            asked = Some(m.clone());
            Ok(" 123456\n".into())
        })
        .await
        .unwrap();
    assert_eq!(asked, Some(MfaMethod::Email));
}

#[tokio::test]
async fn wrong_password_is_reported() {
    let server = MockServer::start().await;
    mock_signin_page(&server).await;
    Mock::given(method("POST"))
        .and(path("/mobile/api/login"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"responseStatus": {"type": "INVALID_USERNAME_PASSWORD"}})),
        )
        .mount(&server)
        .await;

    let client =
        GarminClient::with_endpoints(Endpoints::single_host(&server.uri()), MemoryTokenStore::new()).unwrap();
    let err =
        client.login_with_options("me@example.com", "bad", &fast(), |_| unreachable!()).await.unwrap_err();
    assert!(matches!(err, Error::InvalidCredentials), "got {err:?}");
}

#[tokio::test]
async fn rate_limit_on_signin_page_is_reported() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/mobile/sso/en_US/sign-in"))
        .respond_with(ResponseTemplate::new(429))
        .mount(&server)
        .await;

    let client =
        GarminClient::with_endpoints(Endpoints::single_host(&server.uri()), MemoryTokenStore::new()).unwrap();
    let err =
        client.login_with_options("me@example.com", "pw", &fast(), |_| unreachable!()).await.unwrap_err();
    assert!(matches!(err, Error::RateLimited(_)), "got {err:?}");
}

#[tokio::test]
async fn not_logged_in_without_tokens() {
    let client =
        GarminClient::with_endpoints(Endpoints::single_host("http://127.0.0.1:9"), MemoryTokenStore::new())
            .unwrap();
    let err = client.hrv("2026-11-07".parse().unwrap()).await.unwrap_err();
    assert!(matches!(err, Error::NotLoggedIn), "got {err:?}");
}

/// A client that is already logged in with a token valid for a day.
async fn logged_in(server: &MockServer) -> GarminClient {
    let tokens = Tokens {
        access_token: jwt(far_future(), "CID"),
        refresh_token: Some("r".into()),
        client_id: "CID".into(),
    };
    GarminClient::with_endpoints(Endpoints::single_host(&server.uri()), MemoryTokenStore::with_tokens(tokens))
        .unwrap()
}

#[tokio::test]
async fn fetches_and_parses_wellness_data() {
    let server = MockServer::start().await;
    mock_profile(&server).await;
    Mock::given(method("GET"))
        .and(path("/wellness-service/wellness/dailySleepData/runner%2042"))
        .and(query_param("date", "2026-11-07"))
        .respond_with(ResponseTemplate::new(200).set_body_json(fixture("sleep.json")))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/hrv-service/hrv/2026-11-07"))
        .respond_with(ResponseTemplate::new(200).set_body_json(fixture("hrv.json")))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/wellness-service/wellness/bodyBattery/reports/daily"))
        .and(query_param("startDate", "2026-11-07"))
        .respond_with(ResponseTemplate::new(200).set_body_json(fixture("body_battery.json")))
        .mount(&server)
        .await;

    let client = logged_in(&server).await;
    let date = "2026-11-07".parse().unwrap();

    let sleep = client.sleep(date).await.unwrap().unwrap();
    assert_eq!(sleep.sleep_seconds, Some(26460));
    assert_eq!(sleep.score, Some(81));
    assert_eq!(sleep.score_label.as_deref(), Some("GOOD"));
    assert_eq!(sleep.resting_heart_rate, Some(48));
    assert_eq!(sleep.body_battery_change, Some(54));

    let hrv = client.hrv(date).await.unwrap().unwrap();
    assert_eq!(hrv.last_night_avg, Some(62.0));
    assert_eq!(hrv.status.as_deref(), Some("BALANCED"));
    assert_eq!(hrv.baseline_low, Some(54.0));
    assert_eq!(hrv.baseline_high, Some(68.0));

    let bb = client.body_battery(date).await.unwrap().unwrap();
    assert_eq!(bb.charged, Some(58));
    assert_eq!(bb.highest, Some(88));
    assert_eq!(bb.lowest, Some(31));
    assert_eq!(bb.latest_level, Some(79));
}

#[tokio::test]
async fn fetches_daily_summary_and_training_readiness() {
    let server = MockServer::start().await;
    mock_profile(&server).await;
    Mock::given(method("GET"))
        .and(path("/usersummary-service/usersummary/daily/runner%2042"))
        .and(query_param("calendarDate", "2026-11-07"))
        .respond_with(ResponseTemplate::new(200).set_body_json(fixture("daily_summary.json")))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/metrics-service/metrics/trainingreadiness/2026-11-07"))
        .respond_with(ResponseTemplate::new(200).set_body_json(fixture("training_readiness.json")))
        .mount(&server)
        .await;

    let client = logged_in(&server).await;
    let date = "2026-11-07".parse().unwrap();

    let day = client.daily_summary(date).await.unwrap().unwrap();
    assert_eq!(day.resting_hr, Some(49));
    assert_eq!(day.resting_hr_7d_avg, Some(51));
    assert_eq!(day.avg_stress, Some(27));
    assert_eq!(day.stress_label.as_deref(), Some("BALANCED"));
    assert_eq!(day.high_stress_seconds, Some(1320));
    assert_eq!(day.lowest_spo2, Some(89));
    assert_eq!(day.avg_waking_respiration, Some(15.0));
    assert_eq!(day.body_battery_at_wake, Some(84));
    assert_eq!(day.body_battery_events.len(), 2);
    let run = &day.body_battery_events[1];
    assert_eq!(run.event_type.as_deref(), Some("ACTIVITY"));
    assert_eq!(run.impact, Some(-14));
    assert_eq!(run.duration_seconds, Some(3540));
    assert_eq!(run.activity_name.as_deref(), Some("Morning Run"));
    assert_eq!(run.start_gmt.unwrap().to_rfc3339(), "2026-11-07T15:05:00+00:00");
    // "NONE" feedback is noise and is dropped.
    assert_eq!(day.body_battery_events[0].feedback, None);

    // The morning (after wake-up) entry wins over the later realtime update.
    let tr = client.training_readiness(date).await.unwrap().unwrap();
    assert_eq!(tr.score, Some(68));
    assert_eq!(tr.level.as_deref(), Some("MODERATE"));
    assert_eq!(tr.recovery_time_minutes, Some(1260));
    assert_eq!(tr.factors.len(), 6);
    let hrv = tr.factors.iter().find(|f| f.name == "hrv").unwrap();
    assert_eq!(hrv.percent, Some(90));
    assert_eq!(hrv.feedback.as_deref(), Some("GOOD"));
}

#[tokio::test]
async fn hrv_no_content_is_none() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/hrv-service/hrv/2026-11-07"))
        .respond_with(ResponseTemplate::new(204))
        .mount(&server)
        .await;
    let client = logged_in(&server).await;
    assert!(client.hrv("2026-11-07".parse().unwrap()).await.unwrap().is_none());
}

#[tokio::test]
async fn refreshes_on_401_and_retries() {
    let server = MockServer::start().await;
    let old = jwt(far_future(), "CID");
    let new = jwt(far_future() + 1, "CID");

    Mock::given(method("GET"))
        .and(path("/hrv-service/hrv/2026-11-07"))
        .and(header("authorization", format!("Bearer {old}").as_str()))
        .respond_with(ResponseTemplate::new(401))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/hrv-service/hrv/2026-11-07"))
        .and(header("authorization", format!("Bearer {new}").as_str()))
        .respond_with(ResponseTemplate::new(200).set_body_json(fixture("hrv.json")))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/di-oauth2-service/oauth/token"))
        .and(body_string_contains("grant_type=refresh_token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"access_token": new})))
        .expect(1)
        .mount(&server)
        .await;

    let store = std::sync::Arc::new(MemoryTokenStore::with_tokens(Tokens {
        access_token: old,
        refresh_token: Some("keep-me".into()),
        client_id: "CID".into(),
    }));
    let client =
        GarminClient::with_endpoints(Endpoints::single_host(&server.uri()), SharedStore(store.clone()))
            .unwrap();
    assert!(client.hrv("2026-11-07".parse().unwrap()).await.unwrap().is_some());

    let saved = store.load().unwrap().unwrap();
    assert_eq!(saved.access_token, new);
    // Garmin did not send a new refresh token, so the old one must be kept.
    assert_eq!(saved.refresh_token.as_deref(), Some("keep-me"));
}

#[tokio::test]
async fn refreshes_expiring_token_before_calling() {
    let server = MockServer::start().await;
    let soon = jwt(chrono::Utc::now().timestamp() + 60, "CID");
    let new = jwt(far_future(), "CID");
    Mock::given(method("POST"))
        .and(path("/di-oauth2-service/oauth/token"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"access_token": new, "refresh_token": "r2"})),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/hrv-service/hrv/2026-11-07"))
        .and(header("authorization", format!("Bearer {new}").as_str()))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;

    let tokens = Tokens { access_token: soon, refresh_token: Some("r".into()), client_id: "CID".into() };
    let client = GarminClient::with_endpoints(
        Endpoints::single_host(&server.uri()),
        MemoryTokenStore::with_tokens(tokens),
    )
    .unwrap();
    client.hrv("2026-11-07".parse().unwrap()).await.unwrap();
}

/// Lets a test keep a handle on the store the client writes to.
struct SharedStore(std::sync::Arc<MemoryTokenStore>);

impl TokenStore for SharedStore {
    fn load(&self) -> garmin_connect::Result<Option<Tokens>> {
        self.0.load()
    }
    fn save(&self, tokens: &Tokens) -> garmin_connect::Result<()> {
        self.0.save(tokens)
    }
    fn clear(&self) -> garmin_connect::Result<()> {
        self.0.clear()
    }
}
