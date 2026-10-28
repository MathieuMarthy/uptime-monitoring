# Sub-project 1 — Foundations

Date: 2026-10-28
Status: approved design, pending spec review

## Context

uptime-monitoring is a lightweight, self-hosted alternative to Uptime Kuma: an internet-exposed
instance monitoring ~100 HTTP(S) targets with minimal RAM usage, single binary, SQLite, Docker image.

The current code is a prototype (4 commits, up to `958a85d`). It renders monitor pages but no
worker performs checks, and its data model has structural flaws (no check verdict, second-precision
primary keys, 24h-only retention, two sources of truth for uptime, UUID text keys).

The project is split into 7 sub-projects: 1 Foundations, 2 Authentication, 3 Check engine,
4 Real data & UI, 5 Alerts, 6 Public status pages, 7 Settings & delivery.

This sub-project rebuilds the foundations every later sub-project depends on. Because there is no
production data yet, the `init` migration is rewritten instead of patched.

## Goals

1. Final target database schema for the whole roadmap, in a single rewritten `init` migration.
2. One name everywhere: **monitor** (database, Rust code, routes, UI). The word "service" disappears.
3. A single validated input type for monitor forms, with per-field error messages.
4. Centralized error handling: no silent `unwrap_or_default()`, no SQL error shown to users.
5. Configuration through environment variables.
6. A self-contained binary: static assets embedded, runnable from any working directory.
7. A test base: unit tests for validation, integration tests for routes.
8. A RAM baseline measured and recorded.

## Non-goals

- Authentication, sessions, CSRF, setup flow (sub-project 2).
- Performing checks, writing `monitor_state`, `checks`, `incidents` (sub-project 3).
- Real heartbeat bars, uptime figures, daily rollups, retention purge, auto-refresh, search (sub-project 4).
- Rust code for alerts, status pages, settings UI (sub-projects 5–7). Their tables exist in the
  schema, but no Rust code touches them yet.

## Database schema

All timestamps are Unix epoch **milliseconds** (`INTEGER`). All identifiers are `INTEGER PRIMARY KEY`:
the `checks` table holds ~1M rows (100 monitors × 1/min × 7 days), and integer keys keep rows and
indexes small. Admin pages will be behind authentication and public pages use slugs, so
enumerable ids are not a concern.

Booleans are `INTEGER` 0/1.

### monitors — configuration, written only by the admin

| Column | Type | Constraints / default | Notes |
|---|---|---|---|
| id | INTEGER | PK | |
| name | TEXT | NOT NULL UNIQUE COLLATE NOCASE | |
| kind | TEXT | NOT NULL DEFAULT 'http', CHECK IN ('http') | extension point for future check types |
| url | TEXT | NOT NULL | |
| method | TEXT | NOT NULL DEFAULT 'GET', CHECK IN ('GET','HEAD','POST','PUT','PATCH','DELETE') | |
| headers | TEXT | NULL | JSON object `{"Name": "value"}`, validated on save |
| body | TEXT | NULL | |
| keyword | TEXT | NULL | response body must contain it |
| accepted_statuses | TEXT | NOT NULL DEFAULT '200-299' | e.g. `200-299,301` |
| follow_redirects | INTEGER | NOT NULL DEFAULT 1 | |
| interval_s | INTEGER | NOT NULL DEFAULT 60 | |
| timeout_s | INTEGER | NOT NULL DEFAULT 10 | |
| failure_threshold | INTEGER | NOT NULL DEFAULT 1, CHECK BETWEEN 1 AND 10 | consecutive failures before Down |
| tls_expiry_warn_days | INTEGER | NULL, DEFAULT 14 | NULL = certificate alert disabled |
| paused | INTEGER | NOT NULL DEFAULT 0 | |
| created_at | INTEGER | NOT NULL | |
| updated_at | INTEGER | NOT NULL | |

### monitor_state — runtime state, written only by the check engine (1:1 with monitors)

Kept separate from `monitors` so an admin edit and an engine write never touch the same row.

| Column | Type | Constraints / default |
|---|---|---|
| monitor_id | INTEGER | PK, FK monitors ON DELETE CASCADE |
| status | TEXT | NOT NULL DEFAULT 'pending', CHECK IN ('pending','up','down') |
| consecutive_failures | INTEGER | NOT NULL DEFAULT 0 |
| last_check_at | INTEGER | NULL |
| last_latency_ms | INTEGER | NULL |
| last_error | TEXT | NULL |
| tls_expires_at | INTEGER | NULL |
| tls_warned_for | INTEGER | NULL — expiry date already alerted on, prevents repeated alerts |

A `monitor_state` row is inserted in the same transaction as its monitor.

### checks — raw check results, short retention (default 7 days)

`WITHOUT ROWID`, PK `(monitor_id, checked_at)`: rows are stored clustered by monitor then time, so
"last 24h of a monitor" is a contiguous range scan with no extra index.

| Column | Type | Notes |
|---|---|---|
| monitor_id | INTEGER | FK monitors ON DELETE CASCADE |
| checked_at | INTEGER | ms |
| ok | INTEGER | verdict computed by the engine — single source of truth |
| latency_ms | INTEGER NULL | NULL when no response |
| status_code | INTEGER NULL | |
| error_kind | TEXT NULL | `timeout`, `dns`, `connect`, `tls`, `status`, `keyword` |
| error | TEXT NULL | message, failures only |

### incidents — outage periods, long retention

| Column | Type | Notes |
|---|---|---|
| id | INTEGER | PK |
| monitor_id | INTEGER | NOT NULL, FK ON DELETE CASCADE |
| started_at | INTEGER | NOT NULL — first failed check of the series, not the confirmation time |
| resolved_at | INTEGER | NULL while open |
| error_kind | TEXT | NOT NULL |
| cause | TEXT | NULL |

Indexes: `idx_incidents_monitor (monitor_id, started_at DESC)`; unique partial index
`idx_incidents_open (monitor_id) WHERE resolved_at IS NULL` — the database guarantees at most one
open incident per monitor.

### daily_stats — daily rollups, long retention

`WITHOUT ROWID`, PK `(monitor_id, day)`.

| Column | Type | Notes |
|---|---|---|
| monitor_id | INTEGER | FK ON DELETE CASCADE |
| day | INTEGER | local date of the instance timezone, encoded `YYYYMMDD` |
| monitored_ms | INTEGER | NOT NULL |
| downtime_ms | INTEGER | NOT NULL |
| checks_total | INTEGER | NOT NULL |
| checks_failed | INTEGER | NOT NULL |
| latency_avg_ms | INTEGER NULL | |
| latency_p95_ms | INTEGER NULL | |

**Uptime definition** (used from sub-project 4 on): `1 − downtime / monitored time`.
Each check covers the time elapsed since the previous check, capped at 2 × interval, so paused
periods and periods where the application was down are excluded from monitored time without a
dedicated table. Downtime comes from incidents. A day in a DST change lasts 23 h or 25 h; storing
actual monitored milliseconds handles that.

### notification_channels, monitor_channels, notification_log — alerts

- `notification_channels`: id, name (UNIQUE NOCASE), kind CHECK IN ('webhook','discord','ntfy'),
  config (JSON validated per kind), enabled, created_at.
- `monitor_channels`: (monitor_id, channel_id) PK, both FK ON DELETE CASCADE, `WITHOUT ROWID`.
- `notification_log`: id, channel_id (FK ON DELETE SET NULL), monitor_id (FK ON DELETE SET NULL),
  event CHECK IN ('down','up','tls_expiring','test'), sent_at, ok, error. Purged with `checks`.

Recovery ("up") notifications are always sent; there is no `notify_on_resolved` option.
Channel secrets are stored in plain text in `config` (the database lives on the user's server).

### status_pages, status_page_monitors — public pages

- `status_pages`: id, slug (UNIQUE, `[a-z0-9-]`, validated), title, description, published
  (default 0), created_at, updated_at.
- `status_page_monitors`: (page_id, monitor_id) PK, both FK ON DELETE CASCADE, display_name
  (NULL = monitor name), position. `WITHOUT ROWID`.

### admin, sessions — authentication

- `admin`: id (CHECK id = 1, single row), username, password_hash (Argon2id), updated_at.
- `sessions`: token_hash (SHA-256 of the session token — the token itself is never stored, PK),
  csrf_token, created_at, expires_at. `WITHOUT ROWID`.

### settings — single typed row

| Column | Type | Default |
|---|---|---|
| id | INTEGER | PK, CHECK id = 1 |
| timezone | TEXT | 'UTC' — IANA name; pre-filled from the browser during setup |
| checks_retention_days | INTEGER | 7 |
| history_retention_days | INTEGER NULL | NULL = kept forever (incidents, daily_stats) |
| block_private_targets | INTEGER | 0 |

The migration inserts the single row: `INSERT INTO settings (id) VALUES (1);`

### Removed from the prototype

UUID ids, `services` table name, `current_status` in configuration, `max_retries`
(→ `failure_threshold`), `is_enabled` (→ `paused`, inverted), `expected_status`
(→ `accepted_statuses`), the `metrics` table, `alerts` / `service_alerts`
(→ `notification_channels` / `monitor_channels`), `notify_on_resolved`.

The old migration file is deleted and replaced by `migrations/20261028000000_init.sql`. Local
`uptime.db` files created by the prototype must be deleted (sqlx would report a changed migration).

## Code organization

```
src/
├── main.rs        startup: config → db → router → serve (engine plugs in later)
├── config.rs      Config::from_env(): DATABASE_URL, BIND_ADDR
├── error.rs       AppError + IntoResponse
├── time.rs        now_ms()
├── assets.rs      embedded static files (rust-embed), Content-Type, Cache-Control
├── db.rs          pool + embedded migrations
├── models/
│   ├── monitor.rs         Monitor (config) + CRUD queries
│   └── monitor_state.rs   MonitorState + read queries
├── forms/
│   └── monitor.rs         MonitorForm (raw strings) → MonitorInput (typed) | FieldErrors
├── views/         Askama template structs + view models
│   └── monitors.rs
└── routes/
    ├── health.rs
    └── monitors.rs
```

`views/` replaces the Rust module `src/templates/` (the HTML stays in `templates/` at the project root,
where Askama expects it). The prototype's `models::Monitor` view model moves to `views/`.

Only `monitors` and `monitor_state` get Rust code in this sub-project.

### Configuration

| Variable | Default | |
|---|---|---|
| `DATABASE_URL` | `sqlite://uptime.db` | the Docker image will set `sqlite:///data/uptime.db` |
| `BIND_ADDR` | `0.0.0.0:8080` | |
| `RUST_LOG` | `uptime_monitoring=info,tower_http=info` | |

Invalid values stop startup with a clear message.

### Error handling

`AppError` enum: `NotFound`, `Internal(Box<dyn std::error::Error + Send + Sync>)`, with
`From<sqlx::Error>` (and `From<askama::Error>`) mapping to `Internal`.
- `NotFound` → 404 page rendered with the base layout.
- `Internal` → logged with `tracing::error!` including the source; the user gets a generic 500 page.
  No SQL text reaches the response.
- Handlers return `Result<impl IntoResponse, AppError>`; `?` replaces `unwrap_or_default()`.
- Unique-constraint violation on `monitors.name` is mapped to a field error
  ("This name is already used"), not to a 500.

### Monitor form and validation

`MonitorForm` deserializes every field as `String` (checkboxes as `Option<String>`), so a blank
numeric field never produces Axum's raw 422 response. `MonitorForm::validate()` returns either a
`MonitorInput` (typed, trimmed, empty strings → `None`) or `FieldErrors` (field name → message).
The form template shows each message under its field, and the global banner lists the count.

Rules:

| Field | Rule |
|---|---|
| name | trimmed, 1–100 chars; unique (checked by the database) |
| url | ≤ 2048 chars, parses as URL, scheme `http` or `https`, has a host |
| method | one of GET, HEAD, POST, PUT, PATCH, DELETE |
| accepted_statuses | comma-separated codes or `a-b` ranges, each 100–599, `a ≤ b`, at least one |
| interval_s | integer 5–86400 |
| timeout_s | integer 1–120 and ≤ interval_s |
| failure_threshold | integer 1–10 |
| tls_expiry_warn_days | empty (disabled) or integer 1–365 |
| headers | empty, or a JSON object whose keys are valid HTTP header names and values valid header values (checked with the `http` crate) |
| body | ≤ 64 KiB |
| keyword | ≤ 1 KiB |
| follow_redirects | checkbox |
| active | checkbox; stored inverted as `paused` |

Create inserts `monitors` + `monitor_state` in one transaction and redirects to the detail page.
`toggle-pause` is a single `UPDATE monitors SET paused = 1 - paused, updated_at = ? WHERE id = ?`.
Delete of a missing id returns 404.

### Routes

Unchanged paths, integer ids (`Path<i64>`; a non-integer id returns 404):

`GET /`, `GET|POST /monitors/new`, `GET /monitors/{id}`, `GET|POST /monitors/{id}/edit`,
`POST /monitors/{id}/delete`, `POST /monitors/{id}/toggle-pause`, `GET /health`, `GET /static/*`.

### Pages in this sub-project

- Dashboard: counters Up / Down / Pending / Paused from `monitor_state` and `monitors.paused`;
  cards show name, status, last latency (or "—"); heartbeat bars render 24 gray "no data" ticks.
  No metrics are loaded in memory.
- Detail: configuration (new fields included) and state; uptime shows "—"; incidents section shows
  its empty state.
- Form: new fields (accepted statuses, follow redirects, TLS warning days), per-field errors.

### Static assets

Embedded with `rust-embed` and served under `/static/`, with `Content-Type` from the extension and
`Cache-Control: public, max-age=3600`. In debug builds rust-embed reads from disk, so CSS edits
do not require a rebuild.

### Release profile

```toml
[profile.release]
lto = true
codegen-units = 1
strip = true
```

### RAM baseline

After the release build, run the binary with an empty database and 100 inserted monitors, and record
idle RSS (`ps -o rss`) in the tracking doc. Each later sub-project reports its delta.

## Testing

- Unit tests: `MonitorForm::validate()` — one test per rule (valid and invalid values),
  `accepted_statuses` parser, header JSON validation.
- Integration tests (`tests/`): build the router on an in-memory SQLite pool with migrations, call it
  with `tower::ServiceExt::oneshot`:
  - create a valid monitor → 303 to detail, row in `monitors` and `monitor_state`;
  - create invalid → 200 with field errors, nothing inserted;
  - duplicate name → field error;
  - edit, toggle pause, delete; unknown or non-integer id → 404;
  - `/health` → 200; `/static/css/style.css` → 200 with `text/css`.
- `cargo clippy -- -D warnings` and `cargo test` pass.

## Documentation

Update `docs/architecture.md` (structure, conventions) and rewrite `docs/database-schema.md`
from this schema.

## Acceptance criteria

1. Fresh start creates the database with the schema above and the settings row.
2. Monitor CRUD, pause and delete work through the UI with the new fields.
3. Invalid input shows per-field messages; no 422 or SQL text ever reaches the browser.
4. The release binary runs from any directory and serves its CSS, fonts and images.
5. `cargo build --release`, `cargo clippy -- -D warnings`, `cargo test` pass with no warnings.
6. Idle RSS baseline recorded.

## Decisions recorded for later sub-projects

- **Setup (sub-project 2):** when no admin exists, the app logs a one-time URL
  `/setup?token=…` (random token, regenerated at each start until setup is done). `/setup` returns
  404 without a valid token and disappears once the admin exists. While setup is pending, `/`
  shows a page telling the user to look in the logs (e.g. `docker logs <container>`) for the setup URL.
  The setup page pre-selects the browser timezone (`Intl.DateTimeFormat().resolvedOptions().timeZone`).
- **Timezone (sub-project 4):** daily rollups and all displayed times use `settings.timezone`
  (embedded tz database, `jiff` or `chrono-tz`). Changing it does not rewrite existing rollups.
- **SSRF (sub-project 3/7):** all destinations allowed by default; `block_private_targets`
  blocks private, loopback and link-local addresses after DNS resolution.
