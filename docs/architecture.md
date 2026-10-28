# Architecture

This document describes the project structure and conventions used in uptime-monitoring.

## Tech Stack

- **[Axum](https://github.com/tokio-rs/axum)** — HTTP framework
- **[Askama](https://github.com/askama-rs/askama)** — Compile-time HTML templating (Jinja2-like syntax)
- **[askama_web](https://github.com/askama-rs/askama_web)** — Askama integration for Axum (implements `IntoResponse` for
  templates)
- **[SQLx](https://github.com/launchbadge/sqlx)** — Async SQL toolkit for SQLite, with embedded migrations
- **[rust-embed](https://github.com/pyrossh/rust-embed)** — Static assets embedded in the binary
- **[Tokio](https://tokio.rs/)** — Async runtime

The release binary is self-contained: templates are compiled in, migrations and static assets are embedded. It runs
from any working directory.

## Project Structure

```
uptime-monitoring/
├── docs/                         # Project documentation (specs and plans in docs/superpowers/)
├── migrations/                   # SQL migrations, embedded and applied at startup
├── src/
│   ├── main.rs                   # Entry point: tracing, config, DB pool, serve with graceful shutdown
│   ├── lib.rs                    # `app(state) -> Router`: routes + static assets + 404 fallback + tracing layer
│   ├── config.rs                 # Config::from_env(): DATABASE_URL, BIND_ADDR, TRUST_PROXY_HEADERS
│   ├── error.rs                  # AppError → HTML error pages (404, 405, 403, 429, 500…)
│   ├── auth/                     # Argon2 hashing, random tokens, session cookie
│   │   └── rate_limit.rs         # In-memory limit on failed logins per IP
│   ├── middleware.rs             # require_session (session + CSRF), security headers
│   ├── time.rs                   # now_ms()
│   ├── assets.rs                 # /static/* served from embedded files
│   ├── db.rs                     # SQLite pool (WAL, foreign keys) + migrations
│   ├── state.rs                  # AppState: pool, setup token, login limiter, hashing semaphore
│   ├── models/                   # Entities and their SQL queries
│   │   ├── admin.rs              # The single admin account
│   │   ├── monitor.rs            # Monitor configuration + CRUD
│   │   ├── monitor_state.rs      # Runtime state written by the check engine
│   │   └── session.rs            # Login sessions (token stored as SHA-256)
│   ├── forms/
│   │   ├── auth.rs               # SetupForm, LoginForm
│   │   └── monitor.rs            # MonitorForm (raw strings) → MonitorInput | FieldErrors
│   ├── views/                    # Askama template structs and view models
│   │   ├── auth.rs
│   │   ├── errors.rs
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
  and `/setup?token=…` is printed in the logs. Every page redirects to `/setup`, which only shows the account form with
  the right token. The browser's timezone is sent in a hidden field (`static/js/setup.js`); anything that does not look
  like an IANA name falls back to `UTC`.
- **Passwords.** Argon2id with the `argon2` crate defaults (19 MiB, t=2, p=1), run on the blocking pool, one at a
  time (`AppState::password_hashing`). Login always verifies the password, so timing does not reveal the username.
- **Sessions.** 32 random bytes, hex, in the `__Host-session` cookie (`HttpOnly; Secure; SameSite=Lax; Path=/`),
  valid 30 days from login. Only the SHA-256 is stored. Expired sessions are purged at each login. Because the cookie is
  `Secure`, logging in over plain HTTP only works on `localhost`.
- **Rate limit.** 10 failed logins per IP per 15 minutes, then 429 until the window ends. In memory, at most 10 000 IPs.
- **Headers.** Every response carries a strict CSP, `X-Content-Type-Options: nosniff`, `X-Frame-Options: DENY` and
  `Referrer-Policy: same-origin`; protected pages add `Cache-Control: no-store`. HSTS is left to the reverse proxy.

## Tests

```bash
cargo test                                   # unit + integration tests (in-memory SQLite)
cargo clippy --all-targets -- -D warnings
```

Integration tests build the router on an in-memory database (`tests/common/mod.rs`) and call it with
`tower::ServiceExt::oneshot`; no server or network is involved. `test_app()` is already logged in (`get` sends the
session cookie, `post_form` also adds the CSRF token); `app_with_admin()` and `fresh_app()` cover login and first
launch. The `argon2` and `blake2` crates are built optimized in the dev profile so hashing stays fast in tests.
