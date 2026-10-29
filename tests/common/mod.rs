#![allow(dead_code)]

use axum::Router;
use axum::body::Body;
use axum::http::{HeaderMap, Request, StatusCode, header};
use http_body_util::BodyExt;
use sqlx::SqlitePool;
use tower::ServiceExt;
use uptime_monitoring::models::{Admin, Session};
use uptime_monitoring::state::AppState;

pub const USERNAME: &str = "admin";
pub const PASSWORD: &str = "correct horse battery";

pub struct TestResponse {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub body: String,
}

impl TestResponse {
    /// The `__Host-session` cookie value set by this response, if any.
    pub fn session_cookie(&self) -> Option<String> {
        self.headers
            .get_all(header::SET_COOKIE)
            .iter()
            .filter_map(|v| v.to_str().ok())
            .find_map(|v| v.strip_prefix("__Host-session="))
            .map(|v| v.split(';').next().unwrap_or_default().to_string())
    }
}

/// The router plus the session it sends with `get` and `post_form`, if any.
#[derive(Clone)]
pub struct TestApp {
    pub router: Router,
    pub state: AppState,
    pub session: Option<TestSession>,
}

#[derive(Clone)]
pub struct TestSession {
    pub token: String,
    pub csrf_token: String,
}

impl TestApp {
    pub fn logged_out(&self) -> Self {
        Self {
            session: None,
            ..self.clone()
        }
    }

    pub fn with_session(&self, token: &str, csrf_token: &str) -> Self {
        Self {
            session: Some(TestSession {
                token: token.to_string(),
                csrf_token: csrf_token.to_string(),
            }),
            ..self.clone()
        }
    }
}

async fn pool() -> SqlitePool {
    uptime_monitoring::db::init_pool("sqlite::memory:")
        .await
        .expect("in-memory pool")
}

fn build(state: AppState, session: Option<TestSession>) -> TestApp {
    TestApp {
        router: uptime_monitoring::app(state.clone()),
        state,
        session,
    }
}

/// An app with an admin account and a logged-in session. The admin's password hash is a
/// placeholder: use `app_with_admin` to test logging in.
pub async fn test_app() -> (TestApp, SqlitePool) {
    let pool = pool().await;
    logged_in(AppState::new(pool.clone()), pool).await
}

/// Like `test_app`, with the check engine running: monitor routes really trigger checks.
pub async fn test_app_with_engine() -> (TestApp, SqlitePool) {
    let pool = pool().await;
    let engine = uptime_monitoring::engine::start(
        pool.clone(),
        uptime_monitoring::engine::guard::Guard::new(false),
        uptime_monitoring::notify::Notifier::default(),
    )
    .await
    .expect("engine");
    logged_in(AppState::new(pool.clone()).with_engine(engine), pool).await
}

async fn logged_in(state: AppState, pool: SqlitePool) -> (TestApp, SqlitePool) {
    Admin::setup(&pool, USERNAME, "not-a-real-hash", "UTC")
        .await
        .expect("admin");
    let session = Session::create(&pool).await.expect("session");
    let session = TestSession {
        token: session.token,
        csrf_token: session.csrf_token,
    };
    (build(state, Some(session)), pool)
}

/// An app whose admin can log in with `USERNAME` / `PASSWORD`; no session yet.
pub async fn app_with_admin() -> (TestApp, SqlitePool) {
    let pool = pool().await;
    let hash = uptime_monitoring::auth::hash_password(PASSWORD).expect("hash");
    Admin::setup(&pool, USERNAME, &hash, "UTC")
        .await
        .expect("admin");
    (build(AppState::new(pool.clone()), None), pool)
}

/// A first-launch app with no admin; returns the setup token printed in the logs.
pub async fn fresh_app() -> (TestApp, SqlitePool, String) {
    let pool = pool().await;
    let state = AppState::new(pool.clone());
    let token = state
        .start_setup_if_needed()
        .await
        .expect("setup check")
        .expect("setup token");
    (build(state, None), pool, token)
}

pub async fn send(app: &TestApp, request: Request<Body>) -> TestResponse {
    let response = app
        .router
        .clone()
        .oneshot(request)
        .await
        .expect("infallible");
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    TestResponse {
        status,
        headers,
        body: String::from_utf8_lossy(&bytes).into_owned(),
    }
}

fn with_cookie(
    app: &TestApp,
    builder: axum::http::request::Builder,
) -> axum::http::request::Builder {
    match &app.session {
        Some(s) => builder.header(header::COOKIE, format!("__Host-session={}", s.token)),
        None => builder,
    }
}

pub async fn get(app: &TestApp, uri: &str) -> TestResponse {
    let request = with_cookie(app, Request::get(uri))
        .body(Body::empty())
        .unwrap();
    send(app, request).await
}

/// Posts a form with the session cookie and its CSRF token, when the app has a session.
pub async fn post_form(app: &TestApp, uri: &str, body: &str) -> TestResponse {
    let body = match &app.session {
        Some(s) if body.is_empty() => format!("csrf_token={}", s.csrf_token),
        Some(s) => format!("{body}&csrf_token={}", s.csrf_token),
        None => body.to_string(),
    };
    post_raw(app, uri, &body).await
}

/// Posts a form with the session cookie but exactly the given body (no CSRF token added).
pub async fn post_raw(app: &TestApp, uri: &str, body: &str) -> TestResponse {
    let request = with_cookie(app, Request::post(uri))
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from(body.to_owned()))
        .unwrap();
    send(app, request).await
}

pub async fn login(app: &TestApp, username: &str, password: &str) -> TestResponse {
    post_form(
        &app.logged_out(),
        "/login",
        &format!(
            "username={username}&password={}",
            password.replace(' ', "+")
        ),
    )
    .await
}
