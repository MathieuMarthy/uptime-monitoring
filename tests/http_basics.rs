mod common;

#[tokio::test]
async fn health_ok() {
    let (app, _) = common::test_app().await;
    let r = common::get(&app, "/health").await;
    assert_eq!(r.status, 200);
    assert_eq!(r.body, "healthy");
}

#[tokio::test]
async fn static_css_is_served_with_cache_headers() {
    let (app, _) = common::test_app().await;
    let r = common::get(&app, "/static/css/style.css").await;
    assert_eq!(r.status, 200);
    assert!(
        r.headers["content-type"]
            .to_str()
            .unwrap()
            .starts_with("text/css")
    );
    assert_eq!(r.headers["cache-control"], "public, max-age=3600");
}

#[tokio::test]
async fn unknown_static_file_is_404() {
    let (app, _) = common::test_app().await;
    let r = common::get(&app, "/static/nope.css").await;
    assert_eq!(r.status, 404);
}

#[tokio::test]
async fn unknown_route_renders_404_page() {
    let (app, _) = common::test_app().await;
    let r = common::get(&app, "/nope").await;
    assert_eq!(r.status, 404);
    assert!(r.body.contains("Page not found"));
}

#[tokio::test]
async fn memory_pool_keeps_schema_across_requests() {
    let (app, pool) = common::test_app().await;
    for _ in 0..3 {
        assert_eq!(common::get(&app, "/health").await.status, 200);
    }
    let (count,): (i64,) = sqlx::query_as("SELECT count(*) FROM _sqlx_migrations")
        .fetch_one(&pool)
        .await
        .expect("schema still present");
    assert!(count >= 1);
}

#[tokio::test]
async fn admin_footer_links_the_repo_and_bug_reports() {
    let (app, _) = common::test_app().await;
    let r = common::get(&app, "/").await;
    assert_eq!(r.status, 200);
    let repo = "https://github.com/MathieuMarthy/uptime-monitoring";
    assert!(r.body.contains(&format!(r#"href="{repo}""#)), "{}", r.body);
    assert!(r.body.contains(&format!(
        "{repo}/issues/new?labels=bug&amp;body=Version%3A%20{}",
        env!("CARGO_PKG_VERSION")
    )));
    assert!(r.body.contains(&format!("v{}", env!("CARGO_PKG_VERSION"))));
}

#[tokio::test]
async fn login_page_has_no_admin_footer() {
    let (app, _) = common::app_with_admin().await;
    let r = common::get(&app, "/login").await;
    assert!(!r.body.contains("issues/new"));
}
