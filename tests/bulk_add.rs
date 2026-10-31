mod common;

use common::{TestApp, get, post_form};
use sqlx::SqlitePool;

async fn add_many(app: &TestApp, body: &str) -> common::TestResponse {
    post_form(app, "/monitors/new/bulk", body).await
}

async fn add_channel(pool: &SqlitePool) {
    sqlx::query(
        "INSERT INTO notification_channels (name, kind, config, enabled, created_at)
         VALUES ('Ops', 'webhook', '{\"url\":\"https://hook.example\"}', 1, 0)",
    )
    .execute(pool)
    .await
    .unwrap();
}

async fn names(pool: &SqlitePool) -> Vec<(String, String, Option<String>)> {
    sqlx::query_as("SELECT name, url, group_name FROM monitors ORDER BY id")
        .fetch_all(pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn form_lists_groups_and_channels() {
    let (app, pool) = common::test_app().await;
    add_channel(&pool).await;
    let r = get(&app, "/monitors/new/bulk").await;
    assert_eq!(r.status, 200);
    assert!(r.body.contains(r#"name="urls""#));
    assert!(r.body.contains(r#"name="channel_1" checked"#), "{}", r.body);
}

#[tokio::test]
async fn dashboard_links_to_bulk_add() {
    let (app, _) = common::test_app().await;
    let r = get(&app, "/").await;
    assert!(r.body.contains(r#"href="/monitors/new/bulk""#));
}

#[tokio::test]
async fn creates_one_monitor_per_line_named_after_its_host() {
    let (app, pool) = common::test_app().await;
    let urls = "https://www.example.com/health\n\n  api.example.org  \nhttp://example.net:8080/";
    let body = format!(
        "urls={}&group=Web",
        form_urlencoded::byte_serialize(urls.as_bytes()).collect::<String>()
    );
    let r = add_many(&app, &body).await;
    assert_eq!(r.status, 303, "{}", r.body);
    assert_eq!(r.headers["location"], "/?done=added&count=3");
    let web = Some("Web".to_string());
    assert_eq!(
        names(&pool).await,
        vec![
            (
                "example.com".into(),
                "https://www.example.com/health".into(),
                web.clone()
            ),
            (
                "api.example.org".into(),
                "https://api.example.org".into(),
                web.clone()
            ),
            ("example.net".into(), "http://example.net:8080/".into(), web),
        ]
    );
    let r = get(&app, "/?done=added&count=3").await;
    assert!(r.body.contains("3 monitors added."));
}

#[tokio::test]
async fn taken_names_get_a_number() {
    let (app, pool) = common::test_app().await;
    add_many(&app, "urls=https%3A%2F%2Fexample.com").await;
    let r = add_many(
        &app,
        "urls=https%3A%2F%2Fexample.com%2Fa%0Ahttps%3A%2F%2FEXAMPLE.com%2Fb",
    )
    .await;
    assert_eq!(r.status, 303, "{}", r.body);
    let got: Vec<String> = names(&pool).await.into_iter().map(|n| n.0).collect();
    assert_eq!(got, ["example.com", "example.com (2)", "example.com (3)"]);
}

#[tokio::test]
async fn one_bad_line_creates_nothing_and_names_the_line() {
    let (app, pool) = common::test_app().await;
    let r = add_many(
        &app,
        "urls=https%3A%2F%2Fexample.com%0Aftp%3A%2F%2Fexample.org&group=Web",
    )
    .await;
    assert_eq!(r.status, 200);
    assert!(r.body.contains("Line 2"), "{}", r.body);
    assert!(r.body.contains("URL must start with http:// or https://."));
    // The paste is kept for a fix.
    assert!(r.body.contains("ftp://example.org"));
    assert!(r.body.contains(r#"value="Web""#));
    assert!(names(&pool).await.is_empty());
}

#[tokio::test]
async fn empty_paste_and_bad_group_are_refused() {
    let (app, pool) = common::test_app().await;
    let r = add_many(&app, "urls=%0A++%0A").await;
    assert_eq!(r.status, 200);
    assert!(r.body.contains("Paste at least one URL."));
    let r = add_many(&app, "urls=example.com&group=ungrouped").await;
    assert_eq!(r.status, 200);
    assert!(r.body.contains("is kept for the monitors without a group."));
    assert!(names(&pool).await.is_empty());
}

#[tokio::test]
async fn too_many_lines_are_refused() {
    let (app, pool) = common::test_app().await;
    let urls: Vec<String> = (0..201).map(|i| format!("https://h{i}.example")).collect();
    let body = format!(
        "urls={}",
        form_urlencoded::byte_serialize(urls.join("\n").as_bytes()).collect::<String>()
    );
    let r = add_many(&app, &body).await;
    assert_eq!(r.status, 200);
    assert!(r.body.contains("At most 200 URLs at once."));
    assert!(names(&pool).await.is_empty());
}

#[tokio::test]
async fn links_ticked_channels_and_starts_checking() {
    let (app, pool) = common::test_app().await;
    add_channel(&pool).await;
    let r = add_many(&app, "urls=a.example%0Ab.example&channel_1=on").await;
    assert_eq!(r.status, 303, "{}", r.body);
    let links: i64 =
        sqlx::query_scalar("SELECT count(*) FROM monitor_channels WHERE channel_id = 1")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(links, 2);
    let paused: i64 = sqlx::query_scalar("SELECT count(*) FROM monitors WHERE paused = 1")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(paused, 0);
}
