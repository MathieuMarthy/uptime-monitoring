mod common;

use common::{PASSWORD, TestResponse, USERNAME, get, login, post_form, post_raw};
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;

fn assert_redirect(r: &TestResponse, location: &str) {
    assert_eq!(r.status, 303, "body: {}", r.body);
    assert_eq!(r.headers["location"], location);
}

fn setup_body(token: &str, password: &str, confirm: &str, timezone: &str) -> String {
    format!(
        "token={token}&username=admin&password={password}&password_confirm={confirm}&timezone={timezone}"
    )
}

async fn admin_count(pool: &SqlitePool) -> i64 {
    sqlx::query_as::<_, (i64,)>("SELECT count(*) FROM admin")
        .fetch_one(pool)
        .await
        .unwrap()
        .0
}

async fn session_count(pool: &SqlitePool) -> i64 {
    sqlx::query_as::<_, (i64,)>("SELECT count(*) FROM sessions")
        .fetch_one(pool)
        .await
        .unwrap()
        .0
}

// --- First launch ---

#[tokio::test]
async fn before_setup_every_page_leads_to_setup() {
    let (app, _, _) = common::fresh_app().await;
    assert_redirect(&get(&app, "/").await, "/setup");
    assert_redirect(&get(&app, "/login").await, "/setup");
}

#[tokio::test]
async fn setup_without_valid_token_points_to_logs() {
    let (app, _, _) = common::fresh_app().await;
    for uri in ["/setup", "/setup?token=wrong"] {
        let r = get(&app, uri).await;
        assert_eq!(r.status, 200);
        assert!(r.body.contains("server logs"), "{uri}");
        assert!(!r.body.contains("name=\"password\""), "{uri}");
    }
}

#[tokio::test]
async fn setup_with_token_shows_form() {
    let (app, _, token) = common::fresh_app().await;
    let r = get(&app, &format!("/setup?token={token}")).await;
    assert_eq!(r.status, 200);
    assert!(r.body.contains("name=\"password\""));
    assert!(r.body.contains(&format!("value=\"{token}\"")));
}

#[tokio::test]
async fn setup_creates_admin_and_logs_in() {
    let (app, pool, token) = common::fresh_app().await;
    let body = setup_body(
        &token,
        "a+long+password",
        "a+long+password",
        "Europe%2FParis",
    );
    let r = post_form(&app, "/setup", &body).await;
    assert_redirect(&r, "/");

    let cookie = r.headers["set-cookie"].to_str().unwrap();
    for attribute in ["HttpOnly", "Secure", "SameSite=Lax", "Path=/"] {
        assert!(cookie.contains(attribute), "{cookie}");
    }
    assert_eq!(admin_count(&pool).await, 1);
    let (timezone,): (String,) = sqlx::query_as("SELECT timezone FROM settings")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(timezone, "Europe/Paris");

    let token = r.session_cookie().unwrap();
    let app = app.with_session(&token, "unused");
    assert_eq!(get(&app, "/").await.status, 200);
    assert_redirect(&get(&app.logged_out(), "/setup").await, "/login");
}

#[tokio::test]
async fn setup_token_works_only_once() {
    let (app, pool, token) = common::fresh_app().await;
    let body = setup_body(&token, "a+long+password", "a+long+password", "UTC");
    assert_redirect(&post_form(&app, "/setup", &body).await, "/");
    assert_redirect(&post_form(&app, "/setup", &body).await, "/login");
    assert_eq!(admin_count(&pool).await, 1);
}

#[tokio::test]
async fn setup_with_wrong_token_is_forbidden() {
    let (app, pool, _) = common::fresh_app().await;
    let body = setup_body("wrong", "a+long+password", "a+long+password", "UTC");
    assert_eq!(post_form(&app, "/setup", &body).await.status, 403);
    assert_eq!(admin_count(&pool).await, 0);
}

#[tokio::test]
async fn setup_validates_password() {
    let (app, pool, token) = common::fresh_app().await;
    let r = post_form(&app, "/setup", &setup_body(&token, "short", "short", "UTC")).await;
    assert_eq!(r.status, 200);
    assert!(r.body.contains("at least 12 characters"));

    let body = setup_body(&token, "a+long+password", "another+password", "UTC");
    let r = post_form(&app, "/setup", &body).await;
    assert!(r.body.contains("Passwords do not match"));
    assert_eq!(admin_count(&pool).await, 0);
}

#[tokio::test]
async fn setup_ignores_malformed_timezone() {
    let (app, pool, token) = common::fresh_app().await;
    let body = setup_body(&token, "a+long+password", "a+long+password", "%3Cscript%3E");
    assert_redirect(&post_form(&app, "/setup", &body).await, "/");
    let (timezone,): (String,) = sqlx::query_as("SELECT timezone FROM settings")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(timezone, "UTC");
}

// --- Login and sessions ---

#[tokio::test]
async fn protected_pages_redirect_to_login() {
    let (app, pool) = common::test_app().await;
    let app = app.logged_out();
    assert_redirect(&get(&app, "/").await, "/login");
    assert_redirect(&get(&app, "/monitors/new").await, "/login");

    let r = post_form(
        &app,
        "/monitors/new",
        "name=x&url=https%3A%2F%2Fexample.com",
    )
    .await;
    assert_redirect(&r, "/login");
    let (count,): (i64,) = sqlx::query_as("SELECT count(*) FROM monitors")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
}

#[tokio::test]
async fn login_page_renders() {
    let (app, _) = common::app_with_admin().await;
    let r = get(&app, "/login").await;
    assert_eq!(r.status, 200);
    assert!(r.body.contains("name=\"password\""));
}

#[tokio::test]
async fn login_success_stores_only_the_token_hash() {
    let (app, pool) = common::app_with_admin().await;
    let r = login(&app, USERNAME, PASSWORD).await;
    assert_redirect(&r, "/");
    let token = r.session_cookie().unwrap();
    assert_eq!(token.len(), 64);

    let (stored,): (String,) = sqlx::query_as("SELECT token_hash FROM sessions")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_ne!(stored, token);
    let expected: String = Sha256::digest(token.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    assert_eq!(stored, expected);

    assert_eq!(get(&app.with_session(&token, ""), "/").await.status, 200);
}

#[tokio::test]
async fn login_failure_is_generic() {
    let (app, pool) = common::app_with_admin().await;
    for (username, password) in [(USERNAME, "wrong password"), ("someone", PASSWORD)] {
        let r = login(&app, username, password).await;
        assert_eq!(r.status, 200);
        assert!(r.body.contains("Invalid username or password."));
        assert!(r.session_cookie().is_none());
    }
    assert_eq!(session_count(&pool).await, 0);
}

#[tokio::test]
async fn login_page_redirects_when_logged_in() {
    let (app, _) = common::test_app().await;
    assert_redirect(&get(&app, "/login").await, "/");
}

#[tokio::test]
async fn logout_ends_the_session() {
    let (app, pool) = common::test_app().await;
    let r = post_form(&app, "/logout", "").await;
    assert_redirect(&r, "/login");
    assert!(
        r.headers["set-cookie"]
            .to_str()
            .unwrap()
            .contains("Max-Age=0")
    );
    assert_eq!(session_count(&pool).await, 0);
    assert_redirect(&get(&app, "/").await, "/login");
}

#[tokio::test]
async fn expired_session_is_rejected() {
    let (app, pool) = common::test_app().await;
    sqlx::query("UPDATE sessions SET expires_at = 0")
        .execute(&pool)
        .await
        .unwrap();
    assert_redirect(&get(&app, "/").await, "/login");
}

#[tokio::test]
async fn login_purges_expired_sessions() {
    let (app, pool) = common::app_with_admin().await;
    sqlx::query("INSERT INTO sessions VALUES ('old', 'x', 0, 1)")
        .execute(&pool)
        .await
        .unwrap();
    assert_redirect(&login(&app, USERNAME, PASSWORD).await, "/");
    assert_eq!(session_count(&pool).await, 1);
}

#[tokio::test]
async fn login_attempts_are_rate_limited() {
    let (app, _) = common::app_with_admin().await;
    for _ in 0..10 {
        assert_eq!(login(&app, USERNAME, "wrong password").await.status, 200);
    }
    let r = login(&app, USERNAME, PASSWORD).await;
    assert_eq!(r.status, 429);
    assert!(r.session_cookie().is_none());
}

// --- CSRF ---

#[tokio::test]
async fn post_without_or_with_wrong_csrf_token_is_forbidden() {
    let (app, pool) = common::test_app().await;
    let body =
        "name=x&url=https%3A%2F%2Fexample.com&interval_s=60&timeout_s=10&failure_threshold=1";
    assert_eq!(post_raw(&app, "/monitors/new", body).await.status, 403);
    let wrong = format!("{body}&csrf_token=wrong");
    assert_eq!(post_raw(&app, "/monitors/new", &wrong).await.status, 403);
    assert_eq!(post_raw(&app, "/logout", "").await.status, 403);
    assert_eq!(session_count(&pool).await, 1);
}

#[tokio::test]
async fn forms_carry_the_csrf_token() {
    let (app, _) = common::test_app().await;
    let csrf = app.session.clone().unwrap().csrf_token;
    let r = get(&app, "/monitors/new").await;
    assert!(
        r.body
            .contains(&format!("name=\"csrf_token\" value=\"{csrf}\""))
    );
}

// --- Headers ---

#[tokio::test]
async fn security_headers_on_every_response() {
    let (app, _) = common::test_app().await;
    for uri in ["/health", "/login", "/", "/nope"] {
        let r = get(&app.logged_out(), uri).await;
        let csp = r.headers["content-security-policy"].to_str().unwrap();
        assert!(csp.contains("default-src 'self'"), "{uri}");
        assert!(csp.contains("frame-ancestors 'none'"), "{uri}");
        assert_eq!(r.headers["x-content-type-options"], "nosniff", "{uri}");
        assert_eq!(r.headers["x-frame-options"], "DENY", "{uri}");
        assert_eq!(r.headers["referrer-policy"], "same-origin", "{uri}");
    }
}

#[tokio::test]
async fn protected_pages_are_not_cached() {
    let (app, _) = common::test_app().await;
    assert_eq!(get(&app, "/").await.headers["cache-control"], "no-store");
}
