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
│   ├── main.rs                   # Entry point: tracing, config, DB pool, serve with graceful shutdown
│   ├── lib.rs                    # `app(state) -> Router`: routes + static assets + 404 fallback + tracing layer
│   ├── config.rs                 # Config::from_env(): DATABASE_URL, BIND_ADDR, TRUST_PROXY_HEADERS
│   ├── error.rs                  # AppError → HTML error pages (404, 405, 403, 429, 500…)
│   ├── auth/                     # Argon2 hashing, random tokens, session cookie
│   │   └── rate_limit.rs         # In-memory limit on failed logins per IP
│   ├── middleware.rs             # require_session (session + CSRF), security headers
│   ├── time.rs                   # now_ms(), local days and hours, date formatting
│   ├── maintenance.rs            # Hourly upkeep: daily rollups, then purges
│   ├── assets.rs                 # /static/* served from embedded files
│   ├── db.rs                     # SQLite pool (WAL, foreign keys) + migrations
│   ├── state.rs                  # AppState: pool, setup token, login limiter, hashing semaphore, engine handle
│   ├── engine/                   # Check engine
│   │   ├── mod.rs                # start(), supervisor (one task per active monitor), EngineHandle::reload
│   │   ├── scheduler.rs          # The loop of one monitor: check, record, sleep
│   │   ├── http.rs               # One HTTP(S) check → CheckOutcome
│   │   ├── verdict.rs            # Pure pending/up/down state machine
│   │   └── guard.rs              # Private-target blocking (resolver, literal IPs, redirects)
│   ├── models/                   # Entities and their SQL queries
│   │   ├── admin.rs              # The single admin account
│   │   ├── check.rs              # Raw check results
│   │   ├── daily_stat.rs         # Daily rollups
│   │   ├── incident.rs           # Outage periods
│   │   ├── monitor.rs            # Monitor configuration + CRUD
│   │   ├── monitor_state.rs      # Runtime state; record() writes a check, its state and incident
│   │   ├── session.rs            # Login sessions (token stored as SHA-256)
│   │   ├── settings.rs           # The single settings row
│   │   └── stats.rs              # Bucket: availability summed over a time bucket
│   ├── forms/
│   │   ├── auth.rs               # SetupForm, LoginForm
│   │   └── monitor.rs            # MonitorForm (raw strings) → MonitorInput | FieldErrors
│   ├── views/                    # Askama template structs and view models
│   │   ├── auth.rs
│   │   ├── errors.rs
│   │   ├── heartbeat.rs          # Hourly and daily bars, uptime labels
│   │   └── monitors.rs
│   └── routes/                   # Handlers, grouped by domain
│       ├── mod.rs                # Router assembly (public / protected) + parse_id()
│       ├── auth.rs               # /setup, /login, /logout
│       ├── health.rs             # GET /health
│       └── monitors.rs           # /, /monitors/*
├── templates/                    # Askama templates: base.html → layout.html (admin) | public.html
├── static/                       # CSS, JS, fonts, images (embedded at build time)
└── tests/                        # Integration tests against the real router
```

## Conventions

- **Layers.** `models/` owns SQL, `forms/` owns input validation, `views/` owns presentation, `routes/` wires them
  together. Handlers stay thin.
- **Errors.** Handlers return `Result<_, AppError>` and use `?`. `AppError::Internal` is logged with its source; the
  user only sees a generic 500 page. No SQL or framework error text reaches a response.
- **Forms.** Every form field is deserialized as a `String`, so a blank or malformed value becomes a field error shown
  under its input instead of a raw 422. Validation reports all errors at once.
- **Ids.** Integer ids. Path ids are parsed with `routes::parse_id`; anything that is not a positive integer is a 404.
- **Time.** Timestamps are Unix epoch milliseconds (`i64`), from `time::now_ms()`.
- **Protected by default.** `routes::all` merges a protected router, behind `middleware::require_session`, and a
  public one (`/health`, `/setup`, `/login`, static assets). A new route goes in the protected router unless it must be
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
| `RUST_LOG`     | `uptime_monitoring=info,tower_http=info` |

An invalid value stops startup with a message.

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
  series, with that failure's error. The transition is logged and returned, for notifications to hook onto.
- **Recording.** One transaction per check. Its first statement updates `monitor_state` only if the monitor is still
  active and its state is still the one the task knows, so a check that finishes after a pause, an unpause or a
  deletion is dropped and its task ends. A response without TLS clears `tls_expires_at`; a check without a response
  keeps it.
- **Private targets.** With `settings.block_private_targets`, loopback, private, link-local (cloud metadata), CGNAT,
  benchmarking, multicast and reserved addresses are refused, including when embedded in IPv6 (mapped, NAT64, 6to4): by the DNS resolver (the connection uses exactly the checked
  addresses, so DNS rebinding cannot slip through), for literal IP hosts, and on every redirect. The check fails with
  `error_kind = blocked`. The flag is read at startup (`Guard::set_block_private` changes it at runtime).
- **Certificates.** Verified against the system store (`rustls-platform-verifier`): the Docker image needs
  `ca-certificates`, and an internal CA added to the host store is trusted.

## Data and pages

- **Uptime.** Each check stores the time it covers (`covered_ms`: the gap since the previous check, 0 beyond twice
  the interval) and whether that time was downtime (`down`: an incident was open; opening an incident also marks the
  checks of its failure series). Uptime over any window is then `1 − Σ down covered / Σ covered`, one `GROUP BY` on
  the checks (`models::Bucket`): about 35 ms for 100 monitors over 24 h.
- **Local time.** Days and hours are those of `settings.timezone`, read on each request; unknown names fall back to UTC.
- **Maintenance** (`maintenance.rs`, at startup then hourly). Rolls every completed day (5 min after local
  midnight) since the last one rolled up into `daily_stats`, with a nearest-rank p95 latency; then deletes checks and
  the notification log older than `checks_retention_days`, resolved incidents and daily stats older than
  `history_retention_days` (if set), and expired sessions. Rollup runs first, so no day is lost to the purge.
- **Dashboard.** One query for monitors and states (no body or headers), one for the 24 hourly bars of all monitors.
  Bars: gray without checks, green, orange with failures but no downtime, red with downtime. Uptime is truncated,
  never rounded up to 100 %.
- **Detail page.** 24 h and 30 d uptime, 24 h average latency, certificate expiry, 24 hourly and 90 daily bars (daily
  stats, then raw checks for the days not rolled up yet), last 20 incidents and checks.
- **Live refresh.** `static/js/live.js` refetches the page every 30 s while the tab is visible and swaps the elements
  marked `data-live` (each with an id); a redirect (expired session) reloads the page. No extra route or JSON.
- **Search.** `static/js/search.js` filters the cards on `data-search` (name and URL), ignoring case and accents.

## Tests

```bash
cargo test                                   # unit + integration tests (in-memory SQLite)
cargo clippy --all-targets -- -D warnings
```

Integration tests build the router on an in-memory database (`tests/common/mod.rs`) and call it with
`tower::ServiceExt::oneshot`; no server or network is involved. `test_app()` is already logged in (`get` sends the
session cookie, `post_form` also adds the CSRF token); `app_with_admin()` and `fresh_app()` cover login and first
launch; `test_app_with_engine()` also runs the check engine. Engine and HTTP check tests start a local axum server on
`127.0.0.1:0` as their target; `tests/fixtures/cert.der` is a certificate with a known expiry. The `argon2` and `blake2` crates are built optimized in the dev profile so hashing stays fast in tests.
