mod common;

use axum::body::Body;
use axum::http::{Request, header};
use common::{TestApp, TestResponse, get, post_form, send};
use uptime_monitoring::models::Settings;

async fn get_with_language(app: &TestApp, uri: &str, accept: &str) -> TestResponse {
    let request = Request::get(uri)
        .header(header::ACCEPT_LANGUAGE, accept)
        .body(Body::empty())
        .unwrap();
    send(app, request).await
}

fn setup_body(token: &str, language: &str) -> String {
    format!(
        "token={token}&username=admin&password=a+long+password&password_confirm=a+long+password\
         &timezone=UTC&language={language}"
    )
}

#[tokio::test]
async fn setup_page_follows_the_browser_language() {
    let (app, _, token) = common::fresh_app().await;
    let r = get_with_language(&app, &format!("/setup?token={token}"), "fr-FR,fr;q=0.9").await;
    assert!(r.body.contains(r#"<html lang="fr">"#), "{}", r.body);
    assert!(r.body.contains(r#"<option value="fr" selected"#));
}

#[tokio::test]
async fn setup_page_language_can_be_picked() {
    let (app, _, token) = common::fresh_app().await;
    let r = get_with_language(&app, &format!("/setup?token={token}&lang=de"), "fr").await;
    assert!(r.body.contains(r#"<html lang="de">"#));
}

#[tokio::test]
async fn setup_saves_the_language() {
    let (app, pool, token) = common::fresh_app().await;
    let r = post_form(&app, "/setup", &setup_body(&token, "es")).await;
    assert_eq!(r.status, 303, "{}", r.body);
    assert_eq!(Settings::load(&pool).await.unwrap().language, "es");
    // From then on the instance language wins over the browser's.
    let r = get_with_language(&app.logged_out(), "/login", "de").await;
    assert!(r.body.contains(r#"<html lang="es">"#));
}

#[tokio::test]
async fn setup_refuses_unknown_language() {
    let (app, pool, token) = common::fresh_app().await;
    let r = post_form(&app, "/setup", &setup_body(&token, "it")).await;
    assert_eq!(r.status, 200);
    assert_eq!(Settings::load(&pool).await.unwrap().language, "en");
}

#[tokio::test]
async fn settings_change_the_language_live() {
    let (app, pool) = common::test_app().await;
    assert!(
        get(&app, "/settings")
            .await
            .body
            .contains(r#"<html lang="en">"#)
    );
    let r = post_form(&app, "/settings/general", "timezone=UTC&language=fr").await;
    assert_eq!(r.status, 303, "{}", r.body);
    assert_eq!(Settings::load(&pool).await.unwrap().language, "fr");
    let r = get(&app, "/settings?saved=general").await;
    assert!(r.body.contains(r#"<html lang="fr">"#));
    assert!(r.body.contains(r#"<option value="fr" selected"#));
    assert!(r.body.contains("Paramètres enregistrés."));
}

#[tokio::test]
async fn settings_refuse_unknown_language() {
    let (app, pool) = common::test_app().await;
    let r = post_form(&app, "/settings/general", "timezone=UTC&language=xx").await;
    assert_eq!(r.status, 200);
    assert!(r.body.contains("Unknown language."));
    assert_eq!(Settings::load(&pool).await.unwrap().language, "en");
}

#[tokio::test]
async fn pages_errors_and_public_pages_are_translated() {
    let (app, _) = common::test_app().await;
    post_form(&app, "/settings/general", "timezone=UTC&language=fr").await;
    let r = get(&app, "/").await;
    assert!(r.body.contains("Ajouter une sonde"), "{}", r.body);
    assert!(!r.body.contains("Add monitor"));
    let r = get(&app, "/nowhere").await;
    assert_eq!(r.status, 404);
    assert!(r.body.contains("Page introuvable"));
    let r = post_form(&app, "/monitors/new", "name=&url=").await;
    assert!(r.body.contains("Le nom est requis."), "{}", r.body);
}
