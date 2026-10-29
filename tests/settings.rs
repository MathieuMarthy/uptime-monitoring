mod common;

use common::{PASSWORD, TestApp, TestResponse, USERNAME, get, login, post_form};
use sqlx::SqlitePool;
use uptime_monitoring::models::{Session, Settings};

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

fn password_body(current: &str, new: &str, confirm: &str) -> String {
    format!(
        "current_password={}&password={}&password_confirm={}",
        current.replace(' ', "+"),
        new.replace(' ', "+"),
        confirm.replace(' ', "+")
    )
}

// --- Page ---

#[tokio::test]
async fn settings_require_a_session() {
    let (app, _) = common::test_app().await;
    let r = get(&app.logged_out(), "/settings").await;
    assert_redirect(&r, "/login");
}

#[tokio::test]
async fn page_shows_current_values_and_an_estimate() {
    let (app, pool) = common::test_app().await;
    sqlx::query(
        "INSERT INTO monitors (name, name_key, url, interval_s, created_at, updated_at)
         VALUES ('a', 'a', 'https://a.test', 60, 0, 0)",
    )
    .execute(&pool)
    .await
    .unwrap();

    let r = get(&app, "/settings").await;
    assert_eq!(r.status, 200);
    assert!(r.body.contains(r#"<option value="Europe/Paris""#));
    assert!(r.body.contains(r#"<option value="UTC" selected"#));
    assert!(r.body.contains(
        r#"name="checks_retention_days" type="number" min="2" max="365" required value="7""#
    ));
    // 1 440 checks a day × 7 days × 32 bytes.
    assert!(r.body.contains(r#"data-checks-per-day="1440""#));
    assert!(r.body.contains("315 KiB"), "{}", r.body);
}

// --- General ---

#[tokio::test]
async fn timezone_is_saved() {
    let (app, pool) = common::test_app().await;
    let r = post_form(&app, "/settings/general", "timezone=Europe%2FParis").await;
    assert_redirect(&r, "/settings?saved=general");
    assert_eq!(
        Settings::load(&pool).await.unwrap().timezone,
        "Europe/Paris"
    );
    assert!(
        get(&app, "/settings?saved=general")
            .await
            .body
            .contains("Settings saved.")
    );
}

#[tokio::test]
async fn unknown_timezone_is_refused() {
    let (app, pool) = common::test_app().await;
    let r = post_form(&app, "/settings/general", "timezone=Mars%2FOlympus").await;
    assert_eq!(r.status, 200);
    assert!(r.body.contains("Unknown timezone."));
    assert_eq!(Settings::load(&pool).await.unwrap().timezone, "UTC");
}

// --- Data and security ---

#[tokio::test]
async fn retention_and_blocking_are_saved_and_applied() {
    let (app, pool) = common::test_app().await;
    let r = post_form(
        &app,
        "/settings/data",
        "checks_retention_days=30&history_retention_days=365&block_private_targets=on",
    )
    .await;
    assert_redirect(&r, "/settings?saved=data");
    let settings = Settings::load(&pool).await.unwrap();
    assert_eq!(settings.checks_retention_days, 30);
    assert_eq!(settings.history_retention_days, Some(365));
    assert!(settings.block_private_targets);
    assert!(app.state.guard.blocks_private());

    let r = post_form(
        &app,
        "/settings/data",
        "checks_retention_days=7&history_retention_days=",
    )
    .await;
    assert_redirect(&r, "/settings?saved=data");
    let settings = Settings::load(&pool).await.unwrap();
    assert_eq!(settings.history_retention_days, None);
    assert!(!settings.block_private_targets);
    assert!(!app.state.guard.blocks_private());
}

#[tokio::test]
async fn invalid_retention_is_refused() {
    let (app, pool) = common::test_app().await;
    for (body, message) in [
        (
            "checks_retention_days=1&history_retention_days=",
            "between 2 and 365",
        ),
        (
            "checks_retention_days=abc&history_retention_days=",
            "between 2 and 365",
        ),
        (
            "checks_retention_days=30&history_retention_days=10",
            "at least the checks retention",
        ),
        (
            "checks_retention_days=7&history_retention_days=x",
            "a number of days",
        ),
    ] {
        let r = post_form(&app, "/settings/data", body).await;
        assert_eq!(r.status, 200, "{body}");
        assert!(r.body.contains(message), "{body}: {}", r.body);
    }
    assert_eq!(
        Settings::load(&pool).await.unwrap().checks_retention_days,
        7
    );
}

// --- Password ---

#[tokio::test]
async fn password_change_keeps_this_session_and_ends_the_others() {
    let (app, pool) = logged_in_with_password().await;
    let other = Session::create(&pool).await.unwrap();

    let r = post_form(
        &app,
        "/settings/password",
        &password_body(PASSWORD, "a brand new password", "a brand new password"),
    )
    .await;
    assert_redirect(&r, "/settings?saved=password");

    assert_eq!(get(&app, "/settings").await.status, 200);
    assert!(Session::find(&pool, &other.token).await.unwrap().is_none());
    assert_eq!(login(&app, USERNAME, PASSWORD).await.status, 200);
    assert_eq!(
        login(&app, USERNAME, "a brand new password").await.status,
        303
    );
}

#[tokio::test]
async fn wrong_current_password_is_refused() {
    let (app, _) = logged_in_with_password().await;
    let r = post_form(
        &app,
        "/settings/password",
        &password_body(
            "not the password",
            "a brand new password",
            "a brand new password",
        ),
    )
    .await;
    assert_eq!(r.status, 200);
    assert!(r.body.contains("Current password is incorrect."));
    assert_eq!(login(&app, USERNAME, PASSWORD).await.status, 303);
}

#[tokio::test]
async fn new_password_follows_the_setup_rules() {
    let (app, _) = logged_in_with_password().await;
    let r = post_form(
        &app,
        "/settings/password",
        &password_body(PASSWORD, "short", "short"),
    )
    .await;
    assert_eq!(r.status, 200);
    assert!(r.body.contains("at least 12 characters"));
    let r = post_form(
        &app,
        "/settings/password",
        &password_body(PASSWORD, "a brand new password", "another new password"),
    )
    .await;
    assert!(r.body.contains("Passwords do not match."));
}

#[tokio::test]
async fn wrong_current_passwords_count_toward_the_login_limit() {
    let (app, _) = logged_in_with_password().await;
    let body = password_body(
        "not the password",
        "a brand new password",
        "a brand new password",
    );
    for _ in 0..10 {
        assert_eq!(
            post_form(&app, "/settings/password", &body).await.status,
            200
        );
    }
    assert_eq!(
        post_form(&app, "/settings/password", &body).await.status,
        429
    );
}
