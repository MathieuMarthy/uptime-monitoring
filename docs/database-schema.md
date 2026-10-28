# Database Schema

SQLite schema of **uptime-monitoring**. The source of truth is `migrations/20261028000000_init.sql`.

## Engine configuration

- SQLite via SQLx, migrations embedded and applied at startup.
- `WAL` journal mode, `synchronous = NORMAL`, foreign keys enabled, 5 s busy timeout.

## Conventions

- Timestamps are Unix epoch **milliseconds** (`INTEGER`).
- Ids are `INTEGER PRIMARY KEY`: the `checks` table holds about a million rows, and integer keys keep rows and indexes
  small.
- Booleans are `INTEGER` 0/1.
- Configuration and runtime state live in separate tables (`monitors` / `monitor_state`), so an admin edit and a check
  engine write never touch the same row.

## Entity relationships

```mermaid
erDiagram
    MONITORS ||--|| MONITOR_STATE : "runtime state"
    MONITORS ||--o{ CHECKS : "raw results (short retention)"
    MONITORS ||--o{ INCIDENTS : "outages"
    MONITORS ||--o{ DAILY_STATS : "daily rollups"
    MONITORS ||--o{ MONITOR_CHANNELS : ""
    NOTIFICATION_CHANNELS ||--o{ MONITOR_CHANNELS : ""
    NOTIFICATION_CHANNELS ||--o{ NOTIFICATION_LOG : "deliveries"
    STATUS_PAGES ||--o{ STATUS_PAGE_MONITORS : ""
    MONITORS ||--o{ STATUS_PAGE_MONITORS : ""
```

`admin`, `sessions` and `settings` stand alone.

## Tables

### `monitors` — configuration, written by the admin

| Column | Type | Constraints / default | Notes |
|---|---|---|---|
| `id` | INTEGER | PK | |
| `name` | TEXT | NOT NULL UNIQUE COLLATE NOCASE | |
| `kind` | TEXT | NOT NULL DEFAULT `'http'`, CHECK IN (`'http'`) | extension point for future check types |
| `url` | TEXT | NOT NULL | |
| `method` | TEXT | NOT NULL DEFAULT `'GET'`, CHECK IN (GET, HEAD, POST, PUT, PATCH, DELETE) | |
| `headers` | TEXT | NULL | JSON object `{"Name": "value"}` |
| `body` | TEXT | NULL | |
| `keyword` | TEXT | NULL | the response body must contain it |
| `accepted_statuses` | TEXT | NOT NULL DEFAULT `'200-299'` | e.g. `200-299,301` |
| `follow_redirects` | INTEGER | NOT NULL DEFAULT 1 | |
| `interval_s` | INTEGER | NOT NULL DEFAULT 60 | |
| `timeout_s` | INTEGER | NOT NULL DEFAULT 10 | |
| `failure_threshold` | INTEGER | NOT NULL DEFAULT 1, CHECK 1–10 | consecutive failures before Down |
| `tls_expiry_warn_days` | INTEGER | NULL, DEFAULT 14 | NULL = certificate alert disabled |
| `paused` | INTEGER | NOT NULL DEFAULT 0 | |
| `created_at`, `updated_at` | INTEGER | NOT NULL | |

### `monitor_state` — runtime state, written by the check engine (1:1)

`monitor_id` (PK, FK cascade), `status` (`pending` \| `up` \| `down`, default `pending`), `consecutive_failures`,
`last_check_at`, `last_latency_ms`, `last_error`, `tls_expires_at`, `tls_warned_for` (expiry date already alerted on).
A row is created in the same transaction as its monitor.

### `checks` — raw results, short retention (default 7 days)

`WITHOUT ROWID`, PK `(monitor_id, checked_at)`: rows are clustered by monitor then time, so the recent history of one
monitor is a contiguous range scan.

`monitor_id`, `checked_at`, `ok` (verdict computed by the engine — the single source of truth), `latency_ms` (NULL
without a response), `status_code`, `error_kind` (`timeout`, `dns`, `connect`, `tls`, `status`, `keyword`), `error`.

### `incidents` — outage periods, long retention

`id`, `monitor_id`, `started_at` (first failed check of the series), `resolved_at` (NULL while open), `error_kind`,
`cause`.
Indexes: `(monitor_id, started_at DESC)`; unique partial index on `monitor_id WHERE resolved_at IS NULL` — at most one
open incident per monitor.

### `daily_stats` — daily rollups, long retention

`WITHOUT ROWID`, PK `(monitor_id, day)`. `day` is the local date of the instance timezone, encoded `YYYYMMDD`.
Columns: `monitored_ms`, `downtime_ms`, `checks_total`, `checks_failed`, `latency_avg_ms`, `latency_p95_ms`.

**Uptime** = `1 − downtime / monitored time`. Each check covers the time since the previous check, capped at
2 × interval, so paused periods and application downtime are excluded from monitored time.

### Alerts

- `notification_channels`: `id`, `name` (UNIQUE NOCASE), `kind` (`webhook` \| `discord` \| `ntfy`), `config` (JSON,
  validated per kind), `enabled`, `created_at`.
- `monitor_channels`: `(monitor_id, channel_id)` PK, both FK cascade.
- `notification_log`: `id`, `channel_id`, `monitor_id` (both FK `SET NULL`), `event` (`down` \| `up` \|
  `tls_expiring` \| `test`), `sent_at`, `ok`, `error`. Purged with `checks`.

### Public status pages

- `status_pages`: `id`, `slug` (UNIQUE), `title`, `description`, `published`, `created_at`, `updated_at`.
- `status_page_monitors`: `(page_id, monitor_id)` PK, `display_name` (NULL = monitor name), `position`.

### Authentication

- `admin`: single row (`id = 1`), `username`, `password_hash` (Argon2id), `updated_at`.
- `sessions`: `token_hash` (SHA-256 of the session token; the token itself is never stored), `csrf_token`,
  `created_at`, `expires_at`.

### `settings` — single typed row (`id = 1`), created by the migration

| Column | Default |
|---|---|
| `timezone` | `'UTC'` (IANA name; pre-filled from the browser during setup) |
| `checks_retention_days` | 7 |
| `history_retention_days` | NULL = kept forever (incidents, daily stats) |
| `block_private_targets` | 0 |
