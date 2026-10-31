mod common;

use axum::body::Body;
use axum::http::{Request, header};
use common::{PASSWORD, TestApp, TestResponse, USERNAME, get, login, post_form};
use sqlx::SqlitePool;
use uptime_monitoring::auth::totp::{code_at, decode_secret, step_at};
use uptime_monitoring::models::{Admin, Session};
use uptime_monitoring::time::now_ms;

fn assert_redirect(r: &TestResponse, location: &str) {
    assert_eq!(r.status, 303, "body: {}", r.body);
    assert_eq!(r.headers["location"], location);
}

/// An app whose admin really has `PASSWORD`, logged in through `/login`.
async fn logged_in_with_password() -> (TestApp, SqlitePool) {
    let (app, pool) = common::app_with_admin().await;
    let token = login(&app, USERNAME, PASSWORD)
        .await
        .session_cookie()
        .unwrap();
    let csrf = Session::find(&pool, &token)
        .await
        .unwrap()
        .unwrap()
        .csrf_token;
    (app.with_session(&token, &csrf), pool)
}

/// The code of the step `offset` steps from now.
fn code(secret: &str, offset: i64) -> String {
    let secret = decode_secret(secret).unwrap();
    format!("{:06}", code_at(&secret, step_at(now_ms()) + offset))
}

fn password() -> String {
    PASSWORD.replace(' ', "+")
}

fn hidden_secret(body: &str) -> String {
    let start = body.find("name=\"secret\" value=\"").expect("secret field") + 21;
    body[start..start + 32].to_string()
}

/// Turns 2FA on through the settings, with the code of the current step.
async fn enable(app: &TestApp) -> String {
    let secret = hidden_secret(&get(app, "/settings/totp").await.body);
    let r = post_form(
        app,
        "/settings/totp",
        &format!(
            "secret={secret}&current_password={}&code={}",
            password(),
            code(&secret, 0)
        ),
    )
    .await;
    assert_redirect(&r, "/settings?saved=totp");
    secret
}

/// The `__Host-login` cookie a response sets, if any.
fn login_cookie(r: &TestResponse) -> Option<String> {
    r.headers
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find_map(|v| v.strip_prefix("__Host-login="))
        .map(|v| v.split(';').next().unwrap_or_default().to_string())
        .filter(|v| !v.is_empty())
}

async fn totp_page(app: &TestApp, cookie: &str) -> TestResponse {
    let request = Request::get("/login/totp")
        .header(header::COOKIE, format!("__Host-login={cookie}"))
        .body(Body::empty())
        .unwrap();
    common::send(app, request).await
}

async fn submit_code(app: &TestApp, cookie: &str, code: &str) -> TestResponse {
    let request = Request::post("/login/totp")
        .header(header::COOKIE, format!("__Host-login={cookie}"))
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from(format!("code={code}")))
        .unwrap();
    common::send(app, request).await
}

/// Logs in with the password and returns the pending-login cookie.
async fn first_step(app: &TestApp) -> String {
    let r = login(app, USERNAME, PASSWORD).await;
    assert_redirect(&r, "/login/totp");
    assert_eq!(r.session_cookie(), None);
    login_cookie(&r).expect("pending login cookie")
}

// --- Settings ---

#[tokio::test]
async fn settings_show_2fa_off_with_a_setup_link() {
    let (app, _) = logged_in_with_password().await;
    let r = get(&app, "/settings").await;
    assert!(r.body.contains("Two-factor authentication"));
    assert!(r.body.contains("href=\"/settings/totp\""));
}

#[tokio::test]
async fn setup_page_shows_a_new_secret_and_its_qr_code() {
    let (app, _) = logged_in_with_password().await;
    let first = get(&app, "/settings/totp").await;
    assert_eq!(first.status, 200);
    assert!(first.body.contains("<svg class=\"qr-code\""));
    let secret = hidden_secret(&first.body);
    assert!(decode_secret(&secret).is_some());
    assert_ne!(
        secret,
        hidden_secret(&get(&app, "/settings/totp").await.body)
    );
}

#[tokio::test]
async fn wrong_code_does_not_enable_2fa() {
    let (app, _) = logged_in_with_password().await;
    let secret = hidden_secret(&get(&app, "/settings/totp").await.body);
    let wrong = format!(
        "{:06}",
        (code(&secret, 0).parse::<u32>().unwrap() + 1) % 1_000_000
    );
    let r = post_form(
        &app,
        "/settings/totp",
        &format!(
            "secret={secret}&current_password={}&code={wrong}",
            password()
        ),
    )
    .await;
    assert_eq!(r.status, 200);
    assert!(r.body.contains("Invalid code."));
    // The same secret is offered again, so the app already scanned still works.
    assert_eq!(hidden_secret(&r.body), secret);
    assert_redirect(&login(&app, USERNAME, PASSWORD).await, "/");
}

#[tokio::test]
async fn malformed_secret_is_refused() {
    let (app, _) = logged_in_with_password().await;
    let body = format!("secret=MZXW6&current_password={}&code=123456", password());
    let r = post_form(&app, "/settings/totp", &body).await;
    assert_eq!(r.status, 400);
}

#[tokio::test]
async fn enabling_2fa_keeps_this_session_and_ends_the_others() {
    let (app, pool) = logged_in_with_password().await;
    let other = Session::create(&pool).await.unwrap();
    enable(&app).await;
    assert!(Session::find(&pool, &other.token).await.unwrap().is_none());
    let r = get(&app, "/settings").await;
    assert_eq!(r.status, 200);
    assert!(r.body.contains("action=\"/settings/totp/disable\""));
    assert!(!r.body.contains("href=\"/settings/totp\""));
}

#[tokio::test]
async fn disabling_2fa_needs_the_password_and_a_code() {
    let (app, _) = logged_in_with_password().await;
    let secret = enable(&app).await;
    let password = PASSWORD.replace(' ', "+");

    let r = post_form(
        &app,
        "/settings/totp/disable",
        &format!("current_password=wrong&code={}", code(&secret, 1)),
    )
    .await;
    assert_eq!(r.status, 200);
    assert!(r.body.contains("Current password is incorrect."));

    let r = post_form(
        &app,
        "/settings/totp/disable",
        &format!("current_password={password}&code=000000"),
    )
    .await;
    assert_eq!(r.status, 200);
    assert!(r.body.contains("Invalid code."));

    let r = post_form(
        &app,
        "/settings/totp/disable",
        &format!("current_password={password}&code={}", code(&secret, 1)),
    )
    .await;
    assert_redirect(&r, "/settings?saved=totp-off");
    assert_redirect(&login(&app, USERNAME, PASSWORD).await, "/");
}

// --- Login ---

#[tokio::test]
async fn login_with_2fa_asks_for_a_code() {
    let (app, _) = logged_in_with_password().await;
    let secret = enable(&app).await;
    let cookie = first_step(&app).await;

    let r = totp_page(&app, &cookie).await;
    assert_eq!(r.status, 200);
    assert!(r.body.contains("name=\"code\""));

    let r = submit_code(&app, &cookie, "000000").await;
    assert_eq!(r.status, 200);
    assert!(r.body.contains("Invalid code."));
    assert_eq!(r.session_cookie(), None);

    let r = submit_code(&app, &cookie, &code(&secret, 1)).await;
    assert_redirect(&r, "/");
    assert!(r.session_cookie().is_some());
    // The pending login is spent.
    assert_redirect(&totp_page(&app, &cookie).await, "/login");
}

#[tokio::test]
async fn a_code_is_used_only_once() {
    let (app, _) = logged_in_with_password().await;
    let secret = enable(&app).await;
    // The code that enabled 2FA cannot log in.
    let cookie = first_step(&app).await;
    assert_eq!(
        submit_code(&app, &cookie, &code(&secret, 0)).await.status,
        200
    );
    assert_redirect(&submit_code(&app, &cookie, &code(&secret, 1)).await, "/");
    let cookie = first_step(&app).await;
    assert_eq!(
        submit_code(&app, &cookie, &code(&secret, 1)).await.status,
        200
    );
}

#[tokio::test]
async fn code_page_needs_a_pending_login() {
    let (app, _) = logged_in_with_password().await;
    enable(&app).await;
    assert_redirect(&get(&app.logged_out(), "/login/totp").await, "/login");
    assert_redirect(&totp_page(&app, "unknown").await, "/login");
    assert_redirect(&submit_code(&app, "unknown", "123456").await, "/login");
}

#[tokio::test]
async fn five_wrong_codes_end_the_pending_login() {
    let (app, _) = logged_in_with_password().await;
    let secret = enable(&app).await;
    let cookie = first_step(&app).await;
    for _ in 0..4 {
        assert_eq!(submit_code(&app, &cookie, "000000").await.status, 200);
    }
    assert_redirect(&submit_code(&app, &cookie, "000000").await, "/login");
    assert_redirect(
        &submit_code(&app, &cookie, &code(&secret, 1)).await,
        "/login",
    );
}

#[tokio::test]
async fn codes_count_toward_the_login_limit() {
    let (app, _) = logged_in_with_password().await;
    enable(&app).await;
    // The successful login reset the count; each password step and each code takes one attempt.
    let cookie = first_step(&app).await;
    for _ in 0..4 {
        submit_code(&app, &cookie, "000000").await;
    }
    assert_redirect(&submit_code(&app, &cookie, "000000").await, "/login");
    let cookie = first_step(&app).await;
    for _ in 0..3 {
        assert_eq!(submit_code(&app, &cookie, "000000").await.status, 200);
    }
    assert_eq!(submit_code(&app, &cookie, "000000").await.status, 429);
}

#[tokio::test]
async fn enabling_2fa_needs_the_password() {
    let (app, _) = logged_in_with_password().await;
    let secret = hidden_secret(&get(&app, "/settings/totp").await.body);
    let r = post_form(
        &app,
        "/settings/totp",
        &format!(
            "secret={secret}&current_password=wrong&code={}",
            code(&secret, 0)
        ),
    )
    .await;
    assert_eq!(r.status, 200);
    assert!(r.body.contains("Current password is incorrect."));
    assert_eq!(hidden_secret(&r.body), secret);
    assert_redirect(&login(&app, USERNAME, PASSWORD).await, "/");
}

#[tokio::test]
async fn a_password_change_ends_the_pending_login() {
    let (app, pool) = logged_in_with_password().await;
    let secret = enable(&app).await;
    let cookie = first_step(&app).await;
    let hash = uptime_monitoring::auth::hash_password("a brand new password").unwrap();
    sqlx::query("UPDATE admin SET password_hash = ?")
        .bind(hash)
        .execute(&pool)
        .await
        .unwrap();
    assert_redirect(
        &submit_code(&app, &cookie, &code(&secret, 1)).await,
        "/login",
    );
}

#[tokio::test]
async fn turning_2fa_off_between_the_steps_lets_the_login_finish() {
    let (app, pool) = logged_in_with_password().await;
    enable(&app).await;
    let cookie = first_step(&app).await;
    Admin::disable_totp(&pool).await.unwrap();
    let r = submit_code(&app, &cookie, "000000").await;
    assert_redirect(&r, "/");
    assert!(r.session_cookie().is_some());
}
