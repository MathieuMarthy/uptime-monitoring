# Foundations Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Rebuild the prototype's foundations: final database schema, one "monitor" vocabulary, a single validated form type, centralized errors, env config, embedded assets and a test base.

**Architecture:** The crate becomes a library (`src/lib.rs` exposes `app(state) -> Router`) plus a thin `main.rs`, so integration tests in `tests/` drive the real router with `tower::ServiceExt::oneshot` on an in-memory SQLite pool. Layers: `models/` (entities + SQL), `forms/` (raw form → validated input), `views/` (Askama structs + view models), `routes/` (handlers returning `Result<_, AppError>`).

**Tech Stack:** Rust 2024, Axum 0.8, Askama 0.16 + askama_web, SQLx 0.9 (SQLite), rust-embed 8 (`mime-guess` feature), serde_json, url, http 1.

**Spec:** `docs/superpowers/specs/2026-10-28-foundations-design.md`

## Global Constraints

- All timestamps are Unix epoch milliseconds (`i64`), produced by `time::now_ms()`.
- All ids are `INTEGER PRIMARY KEY` / `i64`. No UUID anywhere; the `uuid` dependency is removed.
- The word "service" disappears from code, SQL, routes and UI; the entity is "monitor".
- No SQL text, sqlx error or Axum rejection text ever reaches an HTTP response.
- Handlers never use `unwrap_or_default()` / `unwrap()` on fallible I/O; they return `Result<_, AppError>`.
- Env vars: `DATABASE_URL` (default `sqlite://uptime.db`), `BIND_ADDR` (default `0.0.0.0:8080`), `RUST_LOG` (default `uptime_monitoring=info,tower_http=info`).
- Static assets: served under `/static/`, `Cache-Control: public, max-age=3600`.
- Release profile: `lto = true`, `codegen-units = 1`, `strip = true`.
- Every task ends with `cargo test` and `cargo clippy --all-targets -- -D warnings` passing.

## Review Focus

1. A blank or non-numeric number field (`interval_s=""`, `interval_s=abc`) re-renders the form with a field error — never Axum's raw 422. → Task 4, `blank_number_field_shows_field_error`.
2. A duplicate name differing only in case (`API` vs `api`) is a field error on create and edit, while saving a monitor under its own unchanged name succeeds. → Task 4, `duplicate_name_is_field_error`, `edit_keeping_same_name_succeeds`.
3. Non-integer, negative or unknown ids on every `/monitors/{id}…` route return the HTML 404 page, not Axum's 400 text. → Task 4, `bad_ids_return_404`.
4. After a validation error, the form keeps exactly what the user typed, including unchecked checkboxes. → Task 4, `invalid_form_keeps_user_values`.
5. The in-memory test database survives across requests (a pool that drops its only connection loses the schema). → Task 1, `memory_pool_keeps_schema_across_requests`.

---

### Task 1: Library split, config, errors, embedded assets, test harness

**Files:**
- Create: `src/lib.rs`, `src/config.rs`, `src/error.rs`, `src/time.rs`, `src/assets.rs`, `templates/error.html`, `tests/common/mod.rs`, `tests/http_basics.rs`
- Rename: `src/templates/` → `src/views/` (module `views`); move `src/models/monitor.rs` (the view model `Monitor`, `Tick`) to `src/views/monitor_view.rs` unchanged apart from imports
- Modify: `src/main.rs`, `src/db.rs`, `src/routes/*.rs` (imports `crate::views`), `Cargo.toml`

**Interfaces:**
- Produces:
  - `uptime_monitoring::app(state: AppState) -> axum::Router` — all routes + `/static` + `TraceLayer` + fallback returning `AppError::NotFound`.
  - `config::Config { pub database_url: String, pub bind_addr: std::net::SocketAddr }`, `Config::from_env() -> Result<Config, String>`, `Config::from_lookup(get: impl Fn(&str) -> Option<String>) -> Result<Config, String>`.
  - `time::now_ms() -> i64`.
  - `error::AppError { NotFound, Internal(Box<dyn std::error::Error + Send + Sync>) }`, `impl From<sqlx::Error>`, `impl From<askama::Error>`, `impl IntoResponse`.
  - `views::errors::ErrorTemplate { status: u16, message: &'static str }` (template `error.html`, extends `base.html`).
  - `db::init_pool(database_url: &str) -> Result<SqlitePool, sqlx::Error>`.
  - `assets::router() -> Router<AppState>`.
  - Test helpers in `tests/common/mod.rs`: `test_app() -> (Router, SqlitePool)`, `get(app: &Router, uri: &str) -> TestResponse`, `post_form(app: &Router, uri: &str, body: &str) -> TestResponse`, `struct TestResponse { status: StatusCode, headers: HeaderMap, body: String }`.

- [ ] **Step 1: Cargo changes**

Add `rust-embed = { version = "8.13", features = ["mime-guess"] }`, `serde_json = "1"`, `http = "1"`.
Dev-dependencies: `tower = { version = "0.5", features = ["util"] }`, `http-body-util = "0.1"`.
Add the release profile from Global Constraints.

- [ ] **Step 2: Write the failing tests** (`tests/http_basics.rs`, plus unit tests in `src/config.rs`)

```rust
#[tokio::test] async fn health_ok() {
    let (app, _) = common::test_app().await;
    let r = common::get(&app, "/health").await;
    assert_eq!(r.status, 200); assert_eq!(r.body, "healthy");
}
#[tokio::test] async fn static_css_is_served_with_cache_headers() {
    let r = common::get(&app, "/static/css/style.css").await;
    assert_eq!(r.status, 200);
    assert!(r.headers["content-type"].to_str().unwrap().starts_with("text/css"));
    assert_eq!(r.headers["cache-control"], "public, max-age=3600");
}
#[tokio::test] async fn unknown_static_file_is_404()      // GET /static/nope.css → 404
#[tokio::test] async fn unknown_route_renders_404_page()  // GET /nope → 404, body contains "Page not found"
#[tokio::test] async fn memory_pool_keeps_schema_across_requests() {
    // 3 sequential GET /health, then sqlx::query("SELECT count(*) FROM _sqlx_migrations") on the pool succeeds
}
// src/config.rs
#[test] fn defaults_apply_when_unset()   // from_lookup(|_| None) → "sqlite://uptime.db", 0.0.0.0:8080
#[test] fn invalid_bind_addr_is_error()  // BIND_ADDR="nope" → Err containing "BIND_ADDR"
```

- [ ] **Step 3: Run to verify failure**

Run: `cargo test`
Expected: compile errors (`uptime_monitoring::app`, `common` not found).

- [ ] **Step 4: Implement**

- `src/lib.rs`: declare modules `assets, config, db, error, models, routes, state, time, views` as `pub`; `pub fn app`.
- `src/main.rs`: init tracing (default filter from Global Constraints), `Config::from_env()` (on `Err`, print it and exit with code 1), `init_pool`, `app`, bind `config.bind_addr`, graceful shutdown (keep the existing `shutdown_signal`).
- `db::init_pool`: when the URL contains `:memory:`, use `max_connections(1)`, `min_connections(1)`, `idle_timeout(None)`, `max_lifetime(None)` — an in-memory database disappears with its connection. Otherwise keep the current options.
- `AppError::into_response`: `NotFound` → 404 + `ErrorTemplate { status: 404, message: "Page not found" }`; `Internal(e)` → `tracing::error!(error = %e)` then 500 + `ErrorTemplate { status: 500, message: "Something went wrong. The error has been logged." }`. If rendering the template itself fails, return the plain status code.
- `assets`: `#[derive(rust_embed::Embed)] #[folder = "static/"] struct Static;` handler on `/static/{*path}`; `Content-Type` from `file.metadata.mimetype()`; missing file → `AppError::NotFound`.
- `test_app()`: `init_pool("sqlite::memory:")`, `AppState::new(pool.clone())`, `app(state)`; return both.

- [ ] **Step 5: Run to verify pass**

Run: `cargo test && cargo clippy --all-targets -- -D warnings`
Expected: all tests pass, no warnings. Manually: `cargo run` from `/tmp` (`cd /tmp && cargo run --manifest-path ~/code/uptime-monitoring/Cargo.toml`) serves `/static/css/style.css`.

- [ ] **Step 6: Commit**

```bash
git add -A src tests templates Cargo.toml Cargo.lock
git commit -m "refactor: library split, config, error handling, embedded assets, test harness"
```

---

### Task 2: Monitor form validation

**Files:**
- Create: `src/forms/mod.rs`, `src/forms/monitor.rs`
- Modify: `src/lib.rs` (`pub mod forms`)

**Interfaces:**
- Produces:
  - `#[derive(Debug, Clone, Default, Deserialize)] #[serde(default)] pub struct MonitorForm { pub name, url, method, accepted_statuses, interval_s, timeout_s, failure_threshold, tls_expiry_warn_days, headers, body, keyword: String, pub follow_redirects: Option<String>, pub active: Option<String> }` — field names are the HTML input names.
  - `MonitorForm::new_defaults() -> MonitorForm` — name `""`, url `"https://"`, method `"GET"`, accepted_statuses `"200-299"`, interval_s `"60"`, timeout_s `"10"`, failure_threshold `"1"`, tls_expiry_warn_days `"14"`, follow_redirects and active `Some("on")`.
  - `MonitorForm::validate(&self) -> Result<MonitorInput, FieldErrors>`.
  - `pub type FieldErrors = std::collections::BTreeMap<&'static str, String>;` keyed by the field name.
  - `pub struct MonitorInput { pub name: String, pub url: String, pub method: String, pub headers: Option<String>, pub body: Option<String>, pub keyword: Option<String>, pub accepted_statuses: String, pub follow_redirects: bool, pub interval_s: i64, pub timeout_s: i64, pub failure_threshold: i64, pub tls_expiry_warn_days: Option<i64>, pub paused: bool }`.
  - `pub fn parse_status_ranges(raw: &str) -> Result<Vec<(u16, u16)>, String>`.

- [ ] **Step 1: Write the failing tests** (`#[cfg(test)]` in `src/forms/monitor.rs`)

Helper `fn valid() -> MonitorForm` = `new_defaults()` with name `"API"` and url `"https://example.com/health"`.

```rust
#[test] fn defaults_with_name_and_url_are_valid() {
    let i = valid().validate().unwrap();
    assert_eq!((i.interval_s, i.timeout_s, i.failure_threshold), (60, 10, 1));
    assert_eq!(i.accepted_statuses, "200-299");
    assert_eq!(i.tls_expiry_warn_days, Some(14));
    assert!(i.follow_redirects && !i.paused);
    assert_eq!((i.headers, i.body, i.keyword), (None, None, None));
}
#[test] fn values_are_trimmed()               // name "  API  ", interval " 60 " → "API", 60
#[test] fn name_rules()                       // "" and "   " → errors["name"]; 101 × 'é' → error; 100 × 'é' → ok
#[test] fn url_rules()                        // "ftp://x", "https://", "not a url", 2049-char URL → errors["url"]
#[test] fn method_must_be_known()             // "TRACE" → errors["method"]; each of GET HEAD POST PUT PATCH DELETE ok
#[test] fn interval_bounds()                  // "4", "86401", "", "abc" → errors["interval_s"]; "5", "86400" ok
#[test] fn timeout_bounds()                   // "0", "121" → error; interval 30 + timeout 31 → error; timeout == interval ok
#[test] fn failure_threshold_bounds()         // "0", "11" → error; "1", "10" ok
#[test] fn tls_days_blank_disables()          // "" → tls_expiry_warn_days == None; "0", "366" → error; "1", "365" ok
#[test] fn status_ranges() {
    assert_eq!(parse_status_ranges(" 200-299 , 301 ").unwrap(), vec![(200, 299), (301, 301)]);
    for bad in ["", "299-200", "99", "600", "2xx", "200-", ","] { assert!(parse_status_ranges(bad).is_err()); }
}
#[test] fn accepted_statuses_are_normalized() // " 200 , 301 " → input.accepted_statuses == "200,301"
#[test] fn headers_must_be_object_of_valid_headers() {
    // ok: "", "{}", r#"{"Authorization":"Bearer x"}"#
    // error: "[]", "nope", r#"{"Bad Name":"x"}"#, r#"{"X":"a\nb"}"#, r#"{"X":1}"#
}
#[test] fn body_and_keyword_size_limits()     // body 64*1024+1 bytes → error, 64*1024 ok; keyword 1025 bytes → error
#[test] fn unchecked_boxes_mean_false()       // follow_redirects None → false; active None → paused == true
#[test] fn all_errors_reported_at_once()      // name "" + url "x" + interval "" → errors has the 3 keys
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test forms::`
Expected: compile error, `MonitorForm` not found.

- [ ] **Step 3: Implement `MonitorForm::validate` and `parse_status_ranges`**

Rules are the spec's validation table. Lengths of `name` count `chars()`; `body`/`keyword` limits count bytes. Headers: `serde_json::from_str::<serde_json::Map<String, Value>>`, every value must be a JSON string, names checked with `http::HeaderName::from_bytes`, values with `http::HeaderValue::from_str`. Store the trimmed original JSON text. Empty-after-trim strings become `None`. Validation collects every error before returning.

- [ ] **Step 4: Run to verify pass**

Run: `cargo test && cargo clippy --all-targets -- -D warnings`
Expected: pass.

- [ ] **Step 5: Commit**

```bash
git add src/forms src/lib.rs
git commit -m "feat: monitor form validation with per-field errors"
```

---

### Task 3: New schema and monitor models

**Files:**
- Delete: `migrations/202609061923_init.sql`
- Create: `migrations/20261028000000_init.sql`, `src/models/monitor.rs`, `src/models/monitor_state.rs`, `tests/schema.rs`, `tests/models.rs`
- Modify: `src/models/mod.rs`

The old `service.rs` and `incident.rs` still compile (their SQL is runtime text) and are removed in Task 4. Between Task 3 and Task 4 the monitor pages fail at runtime; only the tests matter. Delete any local `uptime.db*` files: sqlx refuses a database whose applied migration changed.

**Interfaces:**
- Consumes: `MonitorInput` (Task 2), `now_ms()` (Task 1).
- Produces:
  - `#[derive(Debug, Clone, FromRow)] pub struct Monitor { pub id: i64, pub name: String, pub kind: String, pub url: String, pub method: String, pub headers: Option<String>, pub body: Option<String>, pub keyword: Option<String>, pub accepted_statuses: String, pub follow_redirects: bool, pub interval_s: i64, pub timeout_s: i64, pub failure_threshold: i64, pub tls_expiry_warn_days: Option<i64>, pub paused: bool, pub created_at: i64, pub updated_at: i64 }`
  - `Monitor::find_all(pool) -> Result<Vec<Monitor>, sqlx::Error>` (ordered by `name COLLATE NOCASE`), `Monitor::find(pool, id: i64) -> Result<Option<Monitor>, sqlx::Error>`, `Monitor::create(pool, input: &MonitorInput) -> Result<i64, sqlx::Error>` (one transaction: `monitors` row + `monitor_state` row), `Monitor::update(pool, id, input: &MonitorInput) -> Result<bool, sqlx::Error>`, `Monitor::delete(pool, id) -> Result<bool, sqlx::Error>`, `Monitor::toggle_pause(pool, id) -> Result<bool, sqlx::Error>` (single `UPDATE … SET paused = 1 - paused, updated_at = ?`). `bool` = a row was affected.
  - `#[derive(Debug, Clone, Copy, PartialEq, Eq, sqlx::Type)] #[sqlx(type_name = "TEXT", rename_all = "lowercase")] pub enum MonitorStatus { Pending, Up, Down }`
  - `#[derive(Debug, Clone, FromRow)] pub struct MonitorState { pub monitor_id: i64, pub status: MonitorStatus, pub consecutive_failures: i64, pub last_check_at: Option<i64>, pub last_latency_ms: Option<i64>, pub last_error: Option<String>, pub tls_expires_at: Option<i64>, pub tls_warned_for: Option<i64> }`
  - `MonitorState::find_all(pool) -> Result<Vec<MonitorState>, sqlx::Error>`, `MonitorState::find(pool, monitor_id: i64) -> Result<Option<MonitorState>, sqlx::Error>`.
  - `models::is_unique_violation(e: &sqlx::Error) -> bool` (`Database(e) if e.is_unique_violation()`).

- [ ] **Step 1: Write the failing tests**

`tests/schema.rs` (raw SQL on `test_app()`'s pool):
```rust
#[tokio::test] async fn settings_row_has_defaults()       // SELECT … FROM settings → one row: ("UTC", 7, None::<i64>, 0)
#[tokio::test] async fn only_one_open_incident_per_monitor() // 2nd INSERT with resolved_at NULL fails; with resolved_at set, succeeds
#[tokio::test] async fn deleting_monitor_cascades()        // insert monitor + state + check + incident; DELETE monitor → all counts 0
#[tokio::test] async fn settings_is_single_row()           // INSERT INTO settings (id) VALUES (2) fails
#[tokio::test] async fn check_constraints_hold()           // method 'TRACE', failure_threshold 11, status 'paused' → each insert fails
```
`tests/models.rs` (`input()` = `MonitorForm::new_defaults()` with name/url filled, validated):
```rust
#[tokio::test] async fn create_inserts_monitor_and_pending_state() {
    let id = Monitor::create(&pool, &input()).await.unwrap();
    let m = Monitor::find(&pool, id).await.unwrap().unwrap();
    assert_eq!((m.kind.as_str(), m.paused), ("http", false));
    assert!(m.created_at > 1_700_000_000_000);            // milliseconds
    let s = MonitorState::find(&pool, id).await.unwrap().unwrap();
    assert_eq!((s.status, s.consecutive_failures), (MonitorStatus::Pending, 0));
}
#[tokio::test] async fn duplicate_name_ignores_case()      // create "API" then "api" → Err where is_unique_violation(&e)
#[tokio::test] async fn update_changes_fields_and_updated_at() // update interval → 120, updated_at >= created_at; unknown id → Ok(false)
#[tokio::test] async fn toggle_pause_flips()               // false → true → false; unknown id → Ok(false)
#[tokio::test] async fn delete_reports_missing()           // Ok(true) then Ok(false)
#[tokio::test] async fn find_all_sorted_case_insensitively() // "beta", "Alpha" → ["Alpha", "beta"]
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test --test schema --test models`
Expected: FAIL (old schema, missing types).

- [ ] **Step 3: Write the migration**

All tables, columns, defaults, checks, indexes and `WITHOUT ROWID` choices exactly as in the spec's "Database schema" section, including `monitor_state.tls_warned_for`, then `INSERT INTO settings (id) VALUES (1);`.

- [ ] **Step 4: Implement `models/monitor.rs`, `models/monitor_state.rs`, `is_unique_violation`**

- [ ] **Step 5: Run to verify pass**

Run: `cargo test && cargo clippy --all-targets -- -D warnings`
Expected: pass (`http_basics` included).

- [ ] **Step 6: Commit**

```bash
git add -A migrations src/models tests
git commit -m "feat: final database schema and monitor models"
```

---

### Task 4: Routes, views and templates on the new model

**Files:**
- Delete: `src/models/service.rs`, `src/models/incident.rs`, `src/views/monitor_view.rs`
- Create: `tests/monitors.rs`
- Modify: `src/routes/mod.rs`, `src/routes/monitors.rs`, `src/views/monitors.rs`, `src/forms/monitor.rs`, `templates/monitors/{index,detail,form}.html`, `static/css/forms.css`, `Cargo.toml` (remove `uuid`, `chrono`)

**Interfaces:**
- Consumes: Tasks 1–3.
- Produces:
  - `routes::parse_id(raw: &str) -> Result<i64, AppError>` — positive integer or `NotFound`. Handlers take `Path<String>`.
  - `MonitorForm::from_monitor(m: &Monitor) -> MonitorForm` (in `forms/monitor.rs`).
  - `views::monitors::MonitorView { pub monitor: Monitor, pub state: MonitorState }` with `status_label(&self) -> &'static str` ("Paused" if paused, else "Pending" / "Up" / "Down") and `status_class(&self) -> &'static str` (`text-blue`, `text-muted`, `text-green`, `text-red`).
  - `IndexTemplate { monitors: Vec<MonitorView>, up: usize, down: usize, pending: usize, paused: usize }` — paused counts every paused monitor; up/down/pending count only non-paused ones.
  - `DetailTemplate { view: MonitorView }`.
  - `FormTemplate { form: MonitorForm, errors: FieldErrors, action_url: String, cancel_url: String, monitor_id: Option<i64> }` with `is_edit(&self) -> bool` and `error(&self, field: &str) -> Option<&str>`.

- [ ] **Step 1: Write the failing tests** (`tests/monitors.rs`)

`VALID` = url-encoded form with every field of `new_defaults()` plus `name=API&url=https%3A%2F%2Fexample.com`.

```rust
#[tokio::test] async fn create_redirects_to_detail()      // POST /monitors/new VALID → 303, Location "/monitors/1"; GET it → 200, body contains "API"
#[tokio::test] async fn invalid_create_shows_errors_and_inserts_nothing() // name="" → 200, body contains class="field-error"; SELECT count(*) FROM monitors == 0
#[tokio::test] async fn blank_number_field_shows_field_error() // interval_s= and then interval_s=abc → 200 (never 422), field error under interval
#[tokio::test] async fn duplicate_name_is_field_error()   // create "API", create "api" → 200, body contains "already used"; same on edit of a 2nd monitor renamed "API"
#[tokio::test] async fn edit_keeping_same_name_succeeds() // POST /monitors/1/edit with same name, interval 120 → 303; DB interval_s == 120
#[tokio::test] async fn invalid_form_keeps_user_values()  // name="My API", url="x", no follow_redirects, no active → body has value="My API", value="x", and neither checkbox has `checked`
#[tokio::test] async fn toggle_pause_and_delete()         // POST toggle-pause → 303 + paused; POST delete → 303 "/" ; GET /monitors/1 → 404
#[tokio::test] async fn bad_ids_return_404()              // for id in ["abc", "-1", "0", "999", "1.5"]: GET /monitors/{id}, GET …/edit, POST …/edit, POST …/delete, POST …/toggle-pause → 404 with "Page not found"
#[tokio::test] async fn dashboard_counts_states()         // 3 monitors: one paused, one with state set to 'down' via SQL, one pending → body shows counters 0 up, 1 down, 1 pending, 1 paused
#[tokio::test] async fn dashboard_has_no_service_wording() // GET / and GET /monitors/new bodies do not contain "service" (case-insensitive)
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test --test monitors`
Expected: FAIL.

- [ ] **Step 3: Implement routes and views**

Handlers return `Result<Response, AppError>`. Create/update: `form.validate()`; on `Err(errors)` re-render `FormTemplate` with the submitted form; on DB `Err(e)` where `is_unique_violation(&e)`, re-render with `errors["name"] = "This name is already used."`; other errors go through `?`. Successful create/update/toggle redirect (303) to `/monitors/{id}`, delete to `/`. A monitor without its `monitor_state` row is an `AppError::Internal`.

- [ ] **Step 4: Update templates and CSS**

- `index.html`: four counters (Up, Down, Pending, Paused); cards show name, `status_label` with `status_class`, last latency or "—", and `{% for _ in 0..24 %}<span class="tick tick-gray" title="No data yet"></span>{% endfor %}`; the "24h" uptime shows "—".
- `detail.html`: hero status from `status_label`; uptime "—"; configuration lists method, accepted statuses, interval, timeout, failure threshold, follow redirects, TLS warning ("Disabled" when `None`), keyword/headers/body when set; incidents section keeps its empty state.
- `form.html`: inputs renamed to the `MonitorForm` field names; new inputs `accepted_statuses` (text, hint "e.g. 200-299,301"), `follow_redirects` (checkbox), `tls_expiry_warn_days` (number, hint "Leave empty to disable"); checkbox `active` replaces `is_enabled`; under each input `{% if let Some(e) = self.error("<field>") %}<p class="field-error">{{ e }}</p>{% endif %}`; when `errors` is non-empty the banner says "Please fix the {{ errors.len() }} highlighted field(s)."; delete form posts to `/monitors/{id}/delete`.
- `forms.css`: `.field-error { color: var(--text-color-red); font-size: 13px; }`.

- [ ] **Step 5: Run to verify pass**

Run: `cargo test && cargo clippy --all-targets -- -D warnings && grep -rni "service" src templates`
Expected: tests pass, no warnings, grep prints nothing.

- [ ] **Step 6: Manual check**

`rm -f uptime.db* && cargo run`, then in a browser: create, edit (blank interval → field error), pause, delete.

- [ ] **Step 7: Commit**

```bash
git add -A src templates static tests Cargo.toml Cargo.lock
git commit -m "feat: monitor pages on the new schema with per-field validation"
```

---

### Task 5: Documentation and RAM baseline

**Files:**
- Modify: `docs/architecture.md`, `docs/database-schema.md`

- [ ] **Step 1: Rewrite the docs**

`architecture.md`: new tree (spec "Code organization"), layer rules (models own SQL, forms own validation, views own presentation, handlers return `Result<_, AppError>`), env vars, test commands. `database-schema.md`: the spec's schema section, with the mermaid ER diagram updated to the new tables.

- [ ] **Step 2: Full verification**

Run: `cargo build --release && cargo clippy --all-targets -- -D warnings && cargo test`
Expected: all succeed with no warnings.

- [ ] **Step 3: Measure the RAM baseline**

```bash
rm -f /tmp/um-baseline.db*
cd /tmp && DATABASE_URL=sqlite:///tmp/um-baseline.db ~/code/uptime-monitoring/target/release/uptime-monitoring &
sleep 2; ps -o rss= -p $!                                  # idle, empty DB
for i in $(seq 1 100); do curl -s -o /dev/null -X POST localhost:8080/monitors/new \
  --data "name=m$i&url=https%3A%2F%2Fexample.com&method=GET&accepted_statuses=200-299&interval_s=60&timeout_s=10&failure_threshold=1&tls_expiry_warn_days=14&follow_redirects=on&active=on"; done
curl -s -o /dev/null localhost:8080/; ps -o rss= -p $!     # after 100 monitors + one dashboard render
kill $!
```
Record both RSS values (KiB) and the binary size (`ls -l target/release/uptime-monitoring`) in the commit message and in the tracking doc.

- [ ] **Step 4: Commit**

```bash
git add docs
git commit -m "docs: architecture and schema for the foundations"
```
