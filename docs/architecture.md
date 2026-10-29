# Architecture

This document describes the project structure and conventions used in uptime-monitoring.

## Tech Stack

- **[Axum](https://github.com/tokio-rs/axum)** — HTTP framework
- **[Askama](https://github.com/askama-rs/askama)** — Compile-time HTML templating (Jinja2-like syntax)
- **[askama_web](https://github.com/askama-rs/askama_web)** — Askama integration for Axum (implements `IntoResponse` for
  templates)
- **[SQLx](https://github.com/launchbadge/sqlx)** — Async SQL toolkit for SQLite, with embedded migrations
- **[reqwest](https://github.com/seanmonstar/reqwest)** with **rustls** (`ring`) — HTTP client of the check engine;
  certificates are verified against the system store
- **[rust-embed](https://github.com/pyrossh/rust-embed)** — Static assets embedded in the binary
- **[Tokio](https://tokio.rs/)** — Async runtime
- **[jiff](https://github.com/BurntSushi/jiff)** — Timezones, with the tz database embedded (the image needs no
  `/usr/share/zoneinfo`)

The release binary is self-contained: templates are compiled in, migrations and static assets are embedded. It runs
from any working directory.

## Project Structure

```
uptime-monitoring/
├── docs/                         # Project documentation
├── migrations/                   # SQL migrations, embedded and applied at startup
├── src/
│   ├── main.rs                   # Entry point: tracing, config, DB pool, serve with graceful shutdown;
│   │                             # `healthcheck` subcommand
│   ├── lib.rs                    # `app(state) -> Router`: routes + static assets + 404 fallback + tracing layer
│   ├── config.rs                 # Config::from_env(): DATABASE_URL, BIND_ADDR, TRUST_PROXY_HEADERS, PUBLIC_URL
│   ├── healthcheck.rs            # Probe of /health used by the Docker HEALTHCHECK
│   ├── error.rs                  # AppError → HTML error pages (404, 405, 403, 429, 500…)
│   ├── auth/                     # Argon2 hashing, random tokens, session cookie
│   │   └── rate_limit.rs         # In-memory limit on failed logins per IP
│   ├── middleware.rs             # require_session (session + CSRF), security headers
│   ├── time.rs                   # now_ms(), local days and hours, date formatting
│   ├── maintenance.rs            # Hourly upkeep: daily rollups, then purges
│   ├── page_cache.rs             # Rendered public pages, kept 60 s
│   ├── assets.rs                 # /static/* served from embedded files
│   ├── db.rs                     # SQLite pool (WAL, foreign keys) + migrations
│   ├── state.rs                  # AppState: pool, setup token, login limiter, hashing semaphore, engine handle,
│   │                             # guard, notifier, public page cache, PUBLIC_URL
│   ├── engine/                   # Check engine
│   │   ├── mod.rs                # start(), supervisor (one task per active monitor), EngineHandle::reload
│   │   ├── scheduler.rs          # The loop of one monitor: check, record, sleep
│   │   ├── http.rs               # One HTTP(S) check → CheckOutcome
│   │   ├── verdict.rs            # Pure pending/up/down state machine
│   │   └── guard.rs              # Private-target blocking (resolver, literal IPs, redirects)
│   ├── notify/                   # Alerts
│   │   ├── mod.rs                # Notifier: queue, dispatcher, retries, delivery log
│   │   ├── message.rs            # Title and text of an event, whatever the channel
│   │   └── channel.rs            # Webhook / Discord / ntfy config and request
│   ├── models/                   # Entities and their SQL queries
│   │   ├── admin.rs              # The single admin account
│   │   ├── channel.rs            # Notification channels + monitor links
│   │   ├── check.rs              # Raw check results
│   │   ├── daily_stat.rs         # Daily rollups
│   │   ├── incident.rs           # Outage periods
│   │   ├── monitor.rs            # Monitor configuration + CRUD
│   │   ├── monitor_state.rs      # Runtime state; record() writes a check, its state, incident, cert warning
│   │   ├── notification_log.rs   # Deliveries, final result after retries
│   │   ├── session.rs            # Login sessions (token stored as SHA-256)
│   │   ├── settings.rs           # The single settings row; StorageUse for the disk estimate
│   │   ├── stats.rs              # Bucket: availability summed over a time bucket
│   │   └── status_page.rs        # Public pages + their monitors
│   ├── forms/
│   │   ├── auth.rs               # SetupForm, LoginForm
│   │   ├── channel.rs            # ChannelForm → ChannelInput | FieldErrors
│   │   ├── monitor.rs            # MonitorForm (raw strings) → MonitorInput | FieldErrors
│   │   ├── settings.rs           # GeneralForm, DataForm, PasswordForm
│   │   └── status_page.rs        # StatusPageForm → StatusPageInput | FieldErrors
│   ├── views/                    # Askama template structs and view models
│   │   ├── alerts.rs
│   │   ├── auth.rs
│   │   ├── errors.rs
│   │   ├── heartbeat.rs          # Hourly and daily bars, uptime labels
│   │   ├── monitors.rs
│   │   ├── settings.rs           # Settings page, disk estimates
│   │   └── status_pages.rs       # Admin list and form, public page
│   └── routes/                   # Handlers, grouped by domain
│       ├── mod.rs                # Router assembly (public / protected) + parse_id()
│       ├── alerts.rs             # /alerts/*
│       ├── auth.rs               # /setup, /login, /logout
│       ├── health.rs             # GET /health
│       ├── monitors.rs           # /, /monitors/*
│       ├── settings.rs           # /settings/*
│       └── status_pages.rs       # /public-pages/* (admin), /status/{slug} (public)
├── templates/                    # Askama templates: base.html → layout.html (admin) | public.html
├── static/                       # CSS, JS, fonts, images (embedded at build time)
├── tests/                        # Integration tests against the real router
├── Dockerfile                    # musl static binary on distroless/static, non-root
├── compose.yaml                  # Deployment example
└── .github/workflows/ci.yml      # fmt, clippy, tests; image pushed to GHCR on a v* tag
```

## Conventions

- **Layers.** `models/` owns SQL, `forms/` owns input validation, `views/` owns presentation, `routes/` wires them
  together. Handlers stay thin.
- **Errors.** Handlers return `Result<_, AppError>` and use `?`. `AppError::Internal` is logged with its source; the
  user only sees a generic 500 page. No SQL or framework error text reaches a response.
- **Forms.** Every form field is deserialized as a `String`, so a blank or malformed value becomes a field error shown
  under its input instead of a raw 422. Validation reports all errors at once.
- **Ids.** Integer ids. Path ids are parsed with `routes::parse_id` (the `PathId` extractor); anything that is not a positive integer is a 404.
- **Time.** Timestamps are Unix epoch milliseconds (`i64`), from `time::now_ms()`.
- **Protected by default.** `routes::all` merges a protected router, behind `middleware::require_session`, and a
  public one (`/health`, `/setup`, `/login`, `/status/{slug}`, static assets). A new route goes in the protected router unless it must be
  reachable without logging in.
- **CSRF.** `require_session` checks the `csrf_token` field of every non-GET request against the session's token, so
  handlers cannot forget it. Every admin form carries `<input type="hidden" name="csrf_token" value="{{ csrf_token }}">`;
  admin template structs therefore have a `csrf_token` field, taken from the `Extension<Session>`.
- **No inline CSS or JS.** The Content-Security-Policy only allows same-origin files: no `style=` or `on*=` attributes
  and no inline `<script>`. Behaviour goes in `static/js/` (e.g. `data-confirm` handled by `confirm.js`).
- **Vocabulary.** The domain entity is a *monitor* everywhere: database, code, routes and UI.

## Configuration

| Variable       | Default                                 |
|----------------|-----------------------------------------|
| `DATABASE_URL` | `sqlite://uptime.db`                    |
| `BIND_ADDR`    | `0.0.0.0:8080`                          |
| `TRUST_PROXY_HEADERS` | `false`                          |
| `PUBLIC_URL`   | unset                                   |
| `RUST_LOG`     | `uptime_monitoring=info,tower_http=info` |

An invalid value stops startup with a message.

`PUBLIC_URL` is an http(s) origin, without path (routes are root-absolute, so no sub-path deployment), query,
fragment or credentials; the trailing `/` is dropped. When set,
alerts link to `{PUBLIC_URL}/monitors/{id}` and the first-launch setup URL is printed in full.

`TRUST_PROXY_HEADERS=true` takes the client IP (used by the login rate limit) from the last `X-Forwarded-For` entry,
as set by the reverse proxy. Enable it only behind a proxy: otherwise anyone can forge the header. Left off behind a
proxy, every client shares the proxy's IP and the limit becomes global.

## Authentication

- **First launch.** While no admin exists, a one-time token is generated at startup (in memory, new on each restart)
  and `/setup?token=…` is printed on stderr (whatever `RUST_LOG` says). If the admin row disappears while running, the
  next login attempt starts setup again. Every page redirects to `/setup`, which only shows the account form with
  the right token. The browser's timezone is sent in a hidden field (`static/js/setup.js`); anything that does not look
  like an IANA name falls back to `UTC`.
- **Passwords.** Argon2id with the `argon2` crate defaults (19 MiB, t=2, p=1), run on the blocking pool, one at a
  time (`AppState::password_hashing`); a request that waits more than 5 s for its turn gets a 429. Login always verifies the password and compares usernames through their SHA-256, so timing does not reveal the
  username.
- **Sessions.** 32 random bytes, hex, in the `__Host-session` cookie (`HttpOnly; Secure; SameSite=Lax; Path=/`),
  valid 30 days from login. Only the SHA-256 is stored. Expired sessions are purged at each login. Because the cookie is
  `Secure`, logging in over plain HTTP only works on `localhost`; public pages show a warning otherwise
  (`static/js/insecure-warning.js`).
- **Rate limit.** 10 login attempts per client per 15 minutes, then 429 until the window ends; a successful login
  resets the count. Attempts are counted before the password check, so concurrent requests cannot exceed the budget.
  IPv6 clients are keyed on their /64. In memory, at most 10 000 clients: when full, unknown clients are refused until
  entries expire (the table is swept at most once a minute).
- **Headers.** Every response carries a strict CSP, `X-Content-Type-Options: nosniff`, `X-Frame-Options: DENY` and
  `Referrer-Policy: same-origin`; protected pages add `Cache-Control: no-store`. HSTS is left to the reverse proxy.

## Check engine

- **Supervisor.** `engine::start` spawns one task per active monitor, first checks spread at random over
  `min(interval, 60 s)`. Every monitor route calls `state.engine.reload(id)` after a change: the supervisor stops the
  monitor's task, waits for it to end, re-reads the monitor and, unless it is paused or deleted, starts a new task that
  checks at once. At most 20 checks run at the same time.
- **Pace.** The next check starts `interval_s` after the start of the previous one, or `retry_interval_s` while a
  failure series has not reached `failure_threshold`.
- **Check.** One shared client per `(follow_redirects, ignore_tls_errors)` pair, no proxy, no connection reuse (each
  check does DNS + TCP + TLS for real and keeps no idle socket), at most 10 redirects. `timeout_s` bounds the whole
  check, body included. Latency is the time to the response headers. The body is read only when a keyword is set, up
  to 1 MiB. No `Accept-Encoding` is sent. The server certificate's `notAfter` is stored in
  `monitor_state.tls_expires_at`; with redirects, it is the last hop's.
- **State machine** (`verdict.rs`). A success makes the monitor `up` (and resolves the incident if it was `down`);
  `failure_threshold` consecutive failures make it `down` and open an incident dated from the first failure of the
  series, with that failure's error. The transition is logged and turned into alerts.
- **Recording.** One transaction per check. Its first statement updates `monitor_state` only if the monitor is still
  active and its state is still the one the task knows, so a check that finishes after a pause, an unpause or a
  deletion is dropped and its task ends. A response without TLS clears `tls_expires_at`; a check without a response
  keeps it.
- **Certificate warning.** When a check leaves `tls_expires_at` within the monitor's `tls_expiry_warn_days`, the
  same transaction sets `tls_warned_for` to that date: one warning per expiry date, again after a renewal.
- **Private targets.** With `settings.block_private_targets`, loopback, private, link-local (cloud metadata), CGNAT,
  benchmarking, multicast and reserved addresses are refused, including when embedded in IPv6 (mapped, NAT64, 6to4): by the DNS resolver (the connection uses exactly the checked
  addresses, so DNS rebinding cannot slip through), for literal IP hosts, and on every redirect. The check fails with
  `error_kind = blocked`. The flag is read at startup; the settings page changes it at once through the `Guard` shared in `AppState`.
- **Certificates.** Verified against the system store (`rustls-platform-verifier`): the Docker image needs
  `ca-certificates`, and an internal CA added to the host store is trusted.

## Alerts

- **Events.** After recording a check, the scheduler hands the notifier `down` (cause, start of the outage), `up`
  (downtime) and `tls_expiring` events. Only transitions alert: nothing on startup, pause or deletion.
- **Dispatch** (`notify/mod.rs`). A bounded queue (1 000; beyond, alerts are dropped and logged) feeds one task,
  which hands each alert to its monitor's worker (started on the monitor's first alert). A worker sends its alerts one
  after the other, so a retried `down` never lands after the `up` that followed it; the channels of one alert are
  served in parallel, at most 5 requests at a time overall. Each delivery is tried at once, then after 10 s and 60 s;
  a 4xx other than 408/429 is not retried. The final result goes to `notification_log` (channel or monitor NULL if
  deleted meanwhile); errors never include the URL, which holds the webhook's secret. Alerts still queued when the
  process stops are lost.
- **Certificate warning delivery.** `tls_warned_for` is set when the warning is queued, not when it is delivered: a
  warning lost (no channel linked, every attempt failed) is not repeated. An expired certificate still fails the check
  and alerts `down`, unless the monitor ignores TLS errors.
- **Requests.** One client: 10 s timeout, no redirect, no proxy, no idle connection, and the engine's `Guard` (with
  `block_private_targets`, private destinations are refused like check targets). Webhook: JSON
  `{event, monitor: {id, name, url, link} | null, title, message, at}`. Discord: one embed coloured by severity. ntfy:
  JSON publishing to the server root (`topic`, `title`, `message`, `priority` 4 for down, `tags`), bearer token if
  set. With `PUBLIC_URL`, the detail page's address is `monitor.link`, the embed's `url` and ntfy's `click`
  (`link` is `null` without it).
- **UI.** `/alerts` lists channels (monitor count, last delivery) and the last 50 deliveries. The channel form's
  *Send Test* button (`…/new/test`, `…/{id}/edit/test`) sends the channel as typed, once, without saving or logging
  it, and shows the form again with the result; a typed ntfy token is kept in the field. The ntfy token is never sent back to the browser: left blank, the stored one is kept, unless the server changed. Monitors pick their
  channels in their form (checkboxes `channel_<id>`, gathered by `MonitorForm::other`); a new monitor starts with
  every enabled channel checked.

## Data and pages

- **Uptime.** Each check stores the time it covers (`covered_ms`: the gap since the previous check, 0 beyond twice
  the interval) and whether that time was downtime (`down`: an incident was open; opening an incident also marks the
  checks of its failure series). Uptime over any window is then `1 − Σ down covered / Σ covered`, one `GROUP BY` on
  the checks (`models::Bucket`): about 35 ms for 100 monitors over 24 h.
  A check's whole coverage counts in the bucket of its `checked_at`, so long intervals make hourly figures lumpy.
- **Local time.** Days and hours are those of `settings.timezone`, read on each request; unknown names fall back to UTC.
- **Maintenance** (`maintenance.rs`, at startup then hourly). Rolls every completed day since the last one rolled up
  into `daily_stats`, with a nearest-rank p95 latency. A day is complete 5 min after local midnight plus the longest
  failure series of any monitor (`failure_threshold × (retry_interval_s + timeout_s)`), since an incident opening later
  marks earlier checks as downtime. Then it deletes checks and the notification log older than
  `checks_retention_days` (never the checks of a day not rolled up yet), resolved incidents and daily stats older than
  `history_retention_days` (if set), and expired sessions. The schema requires `checks_retention_days ≥ 2` and
  `history_retention_days ≥ checks_retention_days`.
- **Changing the timezone** (from the settings page) resumes the rollup from the last day key: the first day
  in the new zone overlaps the last one (moving east) or leaves hours out (moving west).
- **Dashboard.** One query for monitors and states (no body or headers), one for the 24 hourly bars of all monitors.
  Bars: gray without checks, green, orange with failures but no downtime, red with downtime. Uptime is truncated,
  never rounded up to 100 %.
- **Detail page.** 24 h and 30 d uptime, 24 h average latency, certificate expiry, 24 hourly and 90 daily bars (daily
  stats, then raw checks for the days not rolled up yet), last 20 incidents and checks.
- **Live refresh.** `static/js/live.js` refetches the page every 30 s while the tab is visible and swaps the elements
  marked `data-live` (each with an id); a redirect (expired session) reloads the page. No extra route or JSON.
- **Search.** `static/js/search.js` filters the cards on `data-search` (name and URL), ignoring case and accents.

## Public pages

- **Admin** (`/public-pages`). A page has a title, a slug (`[a-z0-9]+(-[a-z0-9]+)*`, at most 64, unique), an
  optional description and a *published* flag. Its form lists every monitor: checkbox `monitor_<id>`, display name
  `display_name_<id>` (blank = the monitor's name) and order `position_<id>` (0–9999; blank orders last, ties keep
  the form's order, by name). Stored positions are renumbered from 0. `/public-pages/{id}/preview` shows the page,
  published or not, never cached.
- **Public** (`GET /status/{slug}`, no session). An unpublished page is a 404, like an unknown slug. It shows the
  overall state (*Some systems are down* when a monitor that is not paused is down), then per monitor its display
  name, status, 90 daily bars and 90-day uptime, then the last 10 incidents open at some point in the last 90 days, open
  ones first, with their start and duration. A monitor's URL, errors and incident causes are never shown: they can reveal
  internal hosts. `live.js` refreshes it like the admin pages.
- **Cache** (`page_cache.rs`). The rendered HTML is kept 60 s per page id, behind one async mutex per page, so
  concurrent requests wait for a single render: at most one render per page per minute, whatever the traffic. The
  slug lookup still runs on every request (one indexed read), so unpublishing or deleting takes effect at once and
  unknown slugs add no entry; the render reads the page again by id once it holds the slot, so a request racing an
  edit never caches stale data. Editing or deleting a page drops its entry, and expired entries are dropped at the
  next request; other changes (monitor renamed, paused) show within 60 s. Responses carry `Cache-Control: no-cache`:
  browsers and proxies revalidate instead of keeping a page that was unpublished.
- **Daily bars** of several monitors come from `DailyHistory::load_many`: one query on `daily_stats`, then one
  `Bucket::query` per day not rolled up yet, for all of them at once.

## Settings

`/settings` holds three forms, each posted and validated on its own (`/settings/general`, `/settings/data`,
`/settings/password`); success redirects to `/settings?saved=…`.

- **General.** The timezone, chosen among the names of the embedded tz database.
- **Data and security.** Raw checks retention (2–365 days) and history retention (blank = forever, otherwise at least
  the checks retention), applied by the next hourly maintenance; private-target blocking, applied at once.
  Under the fields, a disk estimate: `Σ 86 400 / interval_s` checks a day over the monitors that are not paused × days
  × 32 bytes, and monitors × days × 36 bytes of daily stats (incidents are left out: about 100 bytes each). The sizes
  were measured with `dbstat`; `static/js/settings.js` recomputes them while typing. The page also shows the database
  file's size. Retention does not change memory use.
- **Password.** Current password, new one twice (setup rules). The new password is checked first; then the attempt
  counts toward the client's login limit and the current password is verified on the hashing semaphore. On success,
  every other session is deleted in the same transaction as the new hash.

## Deployment

- **Image.** `Dockerfile` builds a static musl binary (dependencies in their own layer) and copies it onto
  `gcr.io/distroless/static-debian12:nonroot`, which has the CA certificates and runs as uid 65532. `DATABASE_URL`
  points to the `/data` volume. About 21 MB, under 3 MiB of RAM at rest. amd64 only.
- **Health.** No shell in the image: `HEALTHCHECK` runs `uptime-monitoring healthcheck`, which sends `GET /health` to
  `BIND_ADDR` (loopback when it is unspecified) and exits 0 on a 200 within 5 s.
- **Logs.** No colour codes unless stdout is a terminal.
- **SQLite memory.** 512 KiB of page cache per connection (5 connections): with 100 monitors and a week of checks,
  about 5 MiB less RSS than SQLite's 2 MiB default, for the same dashboard time.
- **CI** (`.github/workflows/ci.yml`). On every push to `main` and pull request: `cargo fmt --check`, clippy with
  `-D warnings`, tests, and an image build without push. On a `v*` tag, the image is built and pushed to
  `ghcr.io/<owner>/uptime-monitoring` with the version and `major.minor` tags, plus `latest` for a version that is not
  a pre-release.

## Tests

```bash
cargo test                                   # unit + integration tests (in-memory SQLite)
cargo clippy --all-targets -- -D warnings
```

Integration tests build the router on an in-memory database (`tests/common/mod.rs`) and call it with
`tower::ServiceExt::oneshot`; no server or network is involved. `test_app()` is already logged in (`get` sends the
session cookie, `post_form` also adds the CSRF token); `app_with_admin()` and `fresh_app()` cover login and first
launch; `test_app_with_engine()` also runs the check engine. Engine, HTTP check and alert tests start a local axum server on
`127.0.0.1:0` as their target or receiver (`Notifier::start_with` shortens the retry delays); `tests/fixtures/cert.der` is a certificate with a known expiry. The `argon2` and `blake2` crates are built optimized in the dev profile so hashing stays fast in tests.
