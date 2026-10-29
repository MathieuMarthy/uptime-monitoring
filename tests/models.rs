mod common;

use sqlx::SqlitePool;
use uptime_monitoring::forms::monitor::{MonitorForm, MonitorInput};
use uptime_monitoring::models::{Monitor, MonitorState, MonitorStatus, is_unique_violation};

fn input(name: &str) -> MonitorInput {
    MonitorForm {
        name: name.to_string(),
        url: "https://example.com".to_string(),
        ..MonitorForm::new_defaults()
    }
    .validate()
    .unwrap()
}

async fn pool() -> SqlitePool {
    common::test_app().await.1
}

#[tokio::test]
async fn create_inserts_monitor_and_pending_state() {
    let pool = pool().await;
    let id = Monitor::create(&pool, &input("API")).await.unwrap();
    let m = Monitor::find(&pool, id).await.unwrap().unwrap();
    assert_eq!(
        (m.name.as_str(), m.kind.as_str(), m.paused),
        ("API", "http", false)
    );
    assert_eq!(m.accepted_statuses, "200-299");
    assert!(m.created_at > 1_700_000_000_000);
    let s = MonitorState::find(&pool, id).await.unwrap().unwrap();
    assert_eq!(
        (s.status, s.consecutive_failures),
        (MonitorStatus::Pending, 0)
    );
}

#[tokio::test]
async fn duplicate_name_ignores_case() {
    let pool = pool().await;
    Monitor::create(&pool, &input("API")).await.unwrap();
    let err = Monitor::create(&pool, &input("api")).await.unwrap_err();
    assert!(is_unique_violation(&err));
    let (count,): (i64,) = sqlx::query_as("SELECT count(*) FROM monitor_state")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 1, "failed create must not leave a state row");
}

#[tokio::test]
async fn duplicate_name_ignores_case_of_accented_letters() {
    let pool = pool().await;
    let id = Monitor::create(&pool, &input("Café")).await.unwrap();
    let err = Monitor::create(&pool, &input("CAFÉ")).await.unwrap_err();
    assert!(is_unique_violation(&err));
    let other = Monitor::create(&pool, &input("Other")).await.unwrap();
    let err = Monitor::update(&pool, other, &input("CAFÉ"))
        .await
        .unwrap_err();
    assert!(is_unique_violation(&err));
    assert!(Monitor::update(&pool, id, &input("CAFÉ")).await.unwrap());
}

#[tokio::test]
async fn update_changes_fields_and_updated_at() {
    let pool = pool().await;
    let id = Monitor::create(&pool, &input("API")).await.unwrap();
    let changed = MonitorInput {
        interval_s: 120,
        ..input("API")
    };
    assert!(Monitor::update(&pool, id, &changed).await.unwrap());
    let m = Monitor::find(&pool, id).await.unwrap().unwrap();
    assert_eq!(m.interval_s, 120);
    assert!(m.updated_at >= m.created_at);
    assert!(!Monitor::update(&pool, 999, &changed).await.unwrap());
}

#[tokio::test]
async fn toggle_pause_flips() {
    let pool = pool().await;
    let id = Monitor::create(&pool, &input("API")).await.unwrap();
    assert!(Monitor::toggle_pause(&pool, id).await.unwrap());
    assert!(Monitor::find(&pool, id).await.unwrap().unwrap().paused);
    assert!(Monitor::toggle_pause(&pool, id).await.unwrap());
    assert!(!Monitor::find(&pool, id).await.unwrap().unwrap().paused);
    assert!(!Monitor::toggle_pause(&pool, 999).await.unwrap());
}

#[tokio::test]
async fn delete_reports_missing() {
    let pool = pool().await;
    let id = Monitor::create(&pool, &input("API")).await.unwrap();
    assert!(Monitor::delete(&pool, id).await.unwrap());
    assert!(!Monitor::delete(&pool, id).await.unwrap());
}

#[tokio::test]
async fn find_all_sorted_case_insensitively() {
    let pool = pool().await;
    Monitor::create(&pool, &input("beta")).await.unwrap();
    Monitor::create(&pool, &input("Alpha")).await.unwrap();
    let names: Vec<String> = Monitor::find_all(&pool)
        .await
        .unwrap()
        .into_iter()
        .map(|m| m.name)
        .collect();
    assert_eq!(names, ["Alpha", "beta"]);
}

#[tokio::test]
async fn password_change_is_refused_when_the_hash_or_session_changed() {
    use uptime_monitoring::auth::sha256_hex;
    use uptime_monitoring::models::{Admin, Session};
    let pool = common::test_app().await.1;
    let mine = Session::create(&pool).await.unwrap();
    let mine_hash = sha256_hex(&mine.token);

    // Another change already replaced the hash this request verified.
    assert!(
        !Admin::change_password(&pool, "stale-hash", "new-hash", &mine_hash)
            .await
            .unwrap()
    );
    // This request's session was ended meanwhile.
    assert!(
        !Admin::change_password(&pool, "not-a-real-hash", "new-hash", "gone")
            .await
            .unwrap()
    );
    assert_eq!(
        Admin::find(&pool).await.unwrap().unwrap().password_hash,
        "not-a-real-hash"
    );

    assert!(
        Admin::change_password(&pool, "not-a-real-hash", "new-hash", &mine_hash)
            .await
            .unwrap()
    );
    assert_eq!(
        Admin::find(&pool).await.unwrap().unwrap().password_hash,
        "new-hash"
    );
}
