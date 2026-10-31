mod common;

use sqlx::SqlitePool;
use uptime_monitoring::forms::monitor::MonitorForm;
use uptime_monitoring::models::maintenance::{Maintenance, MaintenanceInput};
use uptime_monitoring::models::{Monitor, Settings};
use uptime_monitoring::windows::{Maintenances, Schedule};

const HOUR: i64 = 3_600_000;

async fn monitor(pool: &SqlitePool, name: &str) -> i64 {
    let input = MonitorForm {
        name: name.to_string(),
        url: "https://example.com".to_string(),
        ..MonitorForm::new_defaults()
    }
    .validate()
    .unwrap();
    Monitor::create(pool, &input).await.unwrap()
}

fn once(title: &str, starts_at: i64, ends_at: i64, monitor_ids: Vec<i64>) -> MaintenanceInput {
    MaintenanceInput {
        title: title.to_string(),
        schedule: Schedule::Once { starts_at, ends_at },
        all_monitors: false,
        show_on_status_pages: false,
        monitor_ids,
    }
}

#[tokio::test]
async fn a_maintenance_is_stored_with_its_monitors() {
    let (_, pool) = common::test_app().await;
    let (a, b) = (monitor(&pool, "A").await, monitor(&pool, "B").await);
    let id = Maintenance::create(&pool, &once("Deploy", 10, 20, vec![a, b]))
        .await
        .unwrap();

    let (found, ids) = Maintenance::find(&pool, id).await.unwrap().unwrap();
    assert_eq!(found.title, "Deploy");
    assert_eq!(
        found.schedule(),
        Schedule::Once {
            starts_at: 10,
            ends_at: 20
        }
    );
    assert_eq!(ids, vec![a, b]);

    let weekly = MaintenanceInput {
        schedule: Schedule::Weekly {
            weekdays: 0b100_0000,
            local_start: 180,
            duration_min: 60,
        },
        all_monitors: true,
        ..once("Backups", 0, 0, vec![b])
    };
    assert!(Maintenance::update(&pool, id, &weekly).await.unwrap());
    let (found, ids) = Maintenance::find(&pool, id).await.unwrap().unwrap();
    assert_eq!(found.schedule(), weekly.schedule);
    assert!(found.all_monitors);
    assert_eq!(ids, vec![b]);

    Monitor::delete(&pool, b).await.unwrap();
    assert_eq!(
        Maintenance::all(&pool).await.unwrap()[0].1,
        Vec::<i64>::new()
    );
    assert!(Maintenance::delete(&pool, id).await.unwrap());
    assert!(Maintenance::find(&pool, id).await.unwrap().is_none());
    assert!(!Maintenance::update(&pool, id, &weekly).await.unwrap());
}

#[tokio::test]
async fn the_shared_set_tells_which_monitors_are_covered() {
    let (_, pool) = common::test_app().await;
    let (a, b) = (monitor(&pool, "A").await, monitor(&pool, "B").await);
    Maintenance::create(&pool, &once("A only", 0, HOUR, vec![a]))
        .await
        .unwrap();
    Maintenance::create(
        &pool,
        &MaintenanceInput {
            all_monitors: true,
            ..once("Everything", 2 * HOUR, 3 * HOUR, vec![])
        },
    )
    .await
    .unwrap();

    let set = Maintenances::default();
    assert!(!set.covers(a, 0), "empty until loaded");
    set.reload(&pool).await.unwrap();
    assert!(set.covers(a, 0));
    assert!(!set.covers(b, 0));
    assert!(!set.covers(a, HOUR));
    assert!(set.covers(b, 2 * HOUR));
    assert_eq!(set.current(a, 30).map(|c| c.ends_at), Some(HOUR));
}

#[tokio::test]
async fn weekly_windows_follow_the_instance_timezone() {
    let (_, pool) = common::test_app().await;
    let a = monitor(&pool, "A").await;
    Maintenance::create(
        &pool,
        &MaintenanceInput {
            schedule: Schedule::Weekly {
                weekdays: 0b111_1111,
                local_start: 120,
                duration_min: 60,
            },
            ..once("Nightly", 0, 0, vec![a])
        },
    )
    .await
    .unwrap();
    Settings::update_timezone(&pool, "Asia/Tokyo")
        .await
        .unwrap();
    let set = Maintenances::default();
    set.reload(&pool).await.unwrap();
    // 02:30 in Tokyo is 17:30 UTC the day before.
    assert!(set.covers(a, 17 * HOUR + HOUR / 2));
    assert!(!set.covers(a, 2 * HOUR + HOUR / 2));
}

#[tokio::test]
async fn ended_one_off_windows_are_purged() {
    let (_, pool) = common::test_app().await;
    let old = Maintenance::create(&pool, &once("Old", 0, 10, vec![]))
        .await
        .unwrap();
    let recent = Maintenance::create(&pool, &once("Recent", 0, 100, vec![]))
        .await
        .unwrap();
    Maintenance::purge_ended_before(&pool, 50).await.unwrap();
    assert!(Maintenance::find(&pool, old).await.unwrap().is_none());
    assert!(Maintenance::find(&pool, recent).await.unwrap().is_some());
}

#[tokio::test]
async fn changing_the_timezone_moves_weekly_windows() {
    let (app, pool) = common::test_app().await;
    let a = monitor(&pool, "A").await;
    Maintenance::create(
        &pool,
        &MaintenanceInput {
            schedule: Schedule::Weekly {
                weekdays: 0b111_1111,
                local_start: 120,
                duration_min: 60,
            },
            ..once("Nightly", 0, 0, vec![a])
        },
    )
    .await
    .unwrap();
    app.state.maintenances.reload(&pool).await.unwrap();
    assert!(app.state.maintenances.covers(a, 2 * HOUR + HOUR / 2));

    common::post_form(&app, "/settings/general", "timezone=Asia%2FTokyo").await;
    assert!(app.state.maintenances.covers(a, 17 * HOUR + HOUR / 2));
}

// --- Pages ---

use common::{TestApp, get, post_form};
use uptime_monitoring::time::now_ms;

fn assert_redirect(r: &common::TestResponse, location: &str) {
    assert_eq!(r.status, 303, "body: {}", r.body);
    assert_eq!(r.headers["location"], location);
}

/// A window on now, for `monitor_ids`, loaded into the app's shared set.
async fn on_now(app: &TestApp, pool: &SqlitePool, monitor_ids: Vec<i64>) -> i64 {
    let now = now_ms();
    let id = Maintenance::create(pool, &once("Deploy", now - HOUR, now + HOUR, monitor_ids))
        .await
        .unwrap();
    app.state.maintenances.reload(pool).await.unwrap();
    id
}

#[tokio::test]
async fn maintenance_pages_need_a_session() {
    let (app, pool) = common::test_app().await;
    assert_redirect(&get(&app.logged_out(), "/maintenances").await, "/login");
    let r = post_form(&app.logged_out(), "/maintenances/new", "title=x&kind=once").await;
    assert_redirect(&r, "/login");
    assert!(Maintenance::all(&pool).await.unwrap().is_empty());
}

#[tokio::test]
async fn a_one_off_window_is_added_edited_and_deleted() {
    let (app, pool) = common::test_app().await;
    let a = monitor(&pool, "Billing").await;
    let r = get(&app, "/maintenances/new").await;
    assert_eq!(r.status, 200);
    assert!(r.body.contains("Billing"));

    let r = post_form(
        &app,
        "/maintenances/new",
        &format!("title=DB+upgrade&kind=once&starts_at=2030-01-02T10%3A00&ends_at=2030-01-02T12%3A30&monitor_{a}=on"),
    )
    .await;
    assert_redirect(&r, "/maintenances");
    let (m, ids) = Maintenance::all(&pool).await.unwrap().remove(0);
    assert_eq!(
        m.schedule(),
        Schedule::Once {
            starts_at: 1_893_578_400_000,
            ends_at: 1_893_587_400_000
        }
    );
    assert_eq!(ids, vec![a]);
    assert!(
        app.state.maintenances.covers(a, 1_893_578_400_000),
        "reloaded"
    );

    let r = get(&app, "/maintenances").await;
    assert!(r.body.contains("DB upgrade"));
    assert!(r.body.contains("2030-01-02 10:00"));
    assert!(r.body.contains("Upcoming"));

    let r = get(&app, &format!("/maintenances/{}/edit", m.id)).await;
    assert!(r.body.contains(r#"value="2030-01-02T10:00""#), "{}", r.body);

    let r = post_form(
        &app,
        &format!("/maintenances/{}/edit", m.id),
        "title=DB+upgrade&kind=once&starts_at=2030-01-02T10%3A00&ends_at=2030-01-02T11%3A00&all_monitors=on",
    )
    .await;
    assert_redirect(&r, "/maintenances");
    let (m, _) = Maintenance::find(&pool, m.id).await.unwrap().unwrap();
    assert!(m.all_monitors);

    let r = post_form(&app, &format!("/maintenances/{}/delete", m.id), "").await;
    assert_redirect(&r, "/maintenances");
    assert!(Maintenance::all(&pool).await.unwrap().is_empty());
    assert!(!app.state.maintenances.covers(a, 1_893_578_400_000));
    assert_eq!(
        post_form(&app, &format!("/maintenances/{}/delete", m.id), "")
            .await
            .status,
        404
    );
}

#[tokio::test]
async fn a_weekly_window_takes_days_a_time_and_a_duration() {
    let (app, pool) = common::test_app().await;
    let a = monitor(&pool, "A").await;
    let r = post_form(
        &app,
        "/maintenances/new",
        &format!("title=Backups&kind=weekly&day_0=on&day_6=on&local_start=02%3A30&duration_min=90&monitor_{a}=on&show_on_status_pages=on"),
    )
    .await;
    assert_redirect(&r, "/maintenances");
    let (m, _) = Maintenance::all(&pool).await.unwrap().remove(0);
    assert_eq!(
        m.schedule(),
        Schedule::Weekly {
            weekdays: 0b100_0001,
            local_start: 150,
            duration_min: 90
        }
    );
    assert!(m.show_on_status_pages);
    let r = get(&app, "/maintenances").await;
    assert!(
        r.body.contains("Mon, Sun at 02:30 for 1 h 30 min"),
        "{}",
        r.body
    );
}

#[tokio::test]
async fn the_form_reports_every_error() {
    let (app, pool) = common::test_app().await;
    let r = post_form(
        &app,
        "/maintenances/new",
        "title=+&kind=once&starts_at=2030-01-02T12%3A00&ends_at=2030-01-02T10%3A00",
    )
    .await;
    assert_eq!(r.status, 200);
    assert!(r.body.contains("Title is required."));
    assert!(r.body.contains("The end must come after the start."));
    assert!(r.body.contains("Pick at least one monitor."));

    let r = post_form(
        &app,
        "/maintenances/new",
        "title=x&kind=weekly&local_start=25%3A00&duration_min=0&all_monitors=on",
    )
    .await;
    assert!(r.body.contains("Pick at least one day."));
    assert!(r.body.contains("Enter a time such as 02:30."));
    assert!(r.body.contains("Duration must be 1 to 10080 minutes."));
    assert!(Maintenance::all(&pool).await.unwrap().is_empty());
}

#[tokio::test]
async fn picking_a_group_picks_its_monitors_when_saved() {
    let (app, pool) = common::test_app().await;
    let web = MonitorForm {
        name: "Web".to_string(),
        url: "https://example.com".to_string(),
        group: "Front".to_string(),
        ..MonitorForm::new_defaults()
    }
    .validate()
    .unwrap();
    let w = Monitor::create(&pool, &web).await.unwrap();
    monitor(&pool, "Other").await;
    let r = post_form(
        &app,
        "/maintenances/new",
        "title=x&kind=once&starts_at=2030-01-02T10%3A00&ends_at=2030-01-02T11%3A00&group_front=on",
    )
    .await;
    assert_redirect(&r, "/maintenances");
    assert_eq!(Maintenance::all(&pool).await.unwrap()[0].1, vec![w]);
}

#[tokio::test]
async fn monitors_in_maintenance_say_so() {
    let (app, pool) = common::test_app().await;
    let a = monitor(&pool, "A").await;
    monitor(&pool, "B").await;
    on_now(&app, &pool, vec![a]).await;

    let r = get(&app, "/").await;
    assert!(r.body.contains("lamp-maintenance"), "{}", r.body);
    assert!(r.body.contains("1 in maintenance"));
    let r = get(&app, &format!("/monitors/{a}")).await;
    assert!(r.body.contains("In maintenance until"), "{}", r.body);
    assert!(r.body.contains("Deploy"));
}

#[tokio::test]
async fn maintenance_now_starts_a_window_on_one_monitor() {
    let (app, pool) = common::test_app().await;
    let a = monitor(&pool, "A").await;
    let before = now_ms();
    let r = post_form(&app, &format!("/monitors/{a}/maintenance"), "minutes=30").await;
    assert_redirect(&r, &format!("/monitors/{a}"));
    let (m, ids) = Maintenance::all(&pool).await.unwrap().remove(0);
    assert_eq!(ids, vec![a]);
    let Schedule::Once { starts_at, ends_at } = m.schedule() else {
        panic!("one-off")
    };
    assert!(starts_at >= before && ends_at - starts_at == 30 * 60_000);
    assert!(app.state.maintenances.covers(a, now_ms()));

    let r = post_form(&app, &format!("/monitors/{a}/maintenance"), "minutes=0").await;
    assert_eq!(r.status, 400);
    let r = post_form(&app, "/monitors/999/maintenance", "minutes=30").await;
    assert_eq!(r.status, 404);
}

#[tokio::test]
async fn status_pages_announce_maintenance() {
    let (app, pool) = common::test_app().await;
    let a = monitor(&pool, "A").await;
    let body = format!("title=S&slug=status&published=on&monitor_{a}=on&position_{a}=1");
    assert_eq!(
        post_form(&app, "/public-pages/new", &body).await.status,
        303
    );
    let now = now_ms();
    // Upcoming but not announced, then announced.
    Maintenance::create(&pool, &once("Quiet", now + HOUR, now + 2 * HOUR, vec![a]))
        .await
        .unwrap();
    app.state.maintenances.reload(&pool).await.unwrap();
    let r = get(&app.logged_out(), "/status/status").await;
    assert!(!r.body.contains("Quiet"));
    assert!(!r.body.contains("Maintenance planned"));
    Maintenance::create(
        &pool,
        &MaintenanceInput {
            show_on_status_pages: true,
            ..once("Network work", now + HOUR, now + 2 * HOUR, vec![a])
        },
    )
    .await
    .unwrap();
    app.state.maintenances.reload(&pool).await.unwrap();
    app.state.public_pages.clear();
    let r = get(&app.logged_out(), "/status/status").await;
    assert!(r.body.contains("Maintenance planned"), "{}", r.body);
    assert!(r.body.contains("Network work"));

    on_now(&app, &pool, vec![a]).await;
    app.state.public_pages.clear();
    let r = get(&app.logged_out(), "/status/status").await;
    assert!(r.body.contains("Maintenance in progress"), "{}", r.body);
}

#[tokio::test]
async fn activity_lists_maintenance_windows() {
    let (app, pool) = common::test_app().await;
    let a = monitor(&pool, "A").await;
    on_now(&app, &pool, vec![a]).await;
    let r = get(&app, "/activity").await;
    assert!(r.body.contains("Maintenance started"), "{}", r.body);
    assert!(r.body.contains("Deploy"));
}

#[tokio::test]
async fn a_new_monitor_does_not_inherit_a_deleted_ones_window() {
    let (app, pool) = common::test_app().await;
    let a = monitor(&pool, "A").await;
    on_now(&app, &pool, vec![a]).await;
    let r = post_form(&app, &format!("/monitors/{a}/delete"), "").await;
    assert_eq!(r.status, 303);
    let b = monitor(&pool, "B").await;
    assert_eq!(a, b, "SQLite reuses the id");
    assert!(!app.state.maintenances.covers(b, now_ms()));
}

#[tokio::test]
async fn activity_shows_no_occurrence_before_a_window_existed() {
    let (app, pool) = common::test_app().await;
    let a = monitor(&pool, "A").await;
    Maintenance::create(
        &pool,
        &MaintenanceInput {
            schedule: Schedule::Weekly {
                weekdays: 0b111_1111,
                // Twelve hours away: not on now, whenever the test runs.
                local_start: (now_ms() / HOUR + 12) % 24 * 60,
                duration_min: 60,
            },
            ..once("Nightly", 0, 0, vec![a])
        },
    )
    .await
    .unwrap();
    let r = get(&app, "/activity?days=30").await;
    assert!(!r.body.contains("Nightly"), "{}", r.body);
}

#[tokio::test]
async fn weekly_occurrences_may_not_overlap() {
    let (app, pool) = common::test_app().await;
    let r = post_form(
        &app,
        "/maintenances/new",
        "title=x&kind=weekly&day_0=on&day_1=on&local_start=00%3A00&duration_min=1441&all_monitors=on",
    )
    .await;
    assert!(
        r.body.contains("Duration must be at most 1440 minutes"),
        "{}",
        r.body
    );
    assert!(Maintenance::all(&pool).await.unwrap().is_empty());
}

#[tokio::test]
async fn maintenance_now_lasts_at_most_four_hours() {
    let (app, pool) = common::test_app().await;
    let a = monitor(&pool, "A").await;
    let r = post_form(&app, &format!("/monitors/{a}/maintenance"), "minutes=241").await;
    assert_eq!(r.status, 400);
    assert!(Maintenance::all(&pool).await.unwrap().is_empty());
}
