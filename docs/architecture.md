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
│   ├── config.rs                 # Config::from_env(): DATABASE_URL, BIND_ADDR
│   ├── error.rs                  # AppError (NotFound, Internal) → 404 / 500 pages
│   ├── time.rs                   # now_ms()
│   ├── assets.rs                 # /static/* served from embedded files
│   ├── db.rs                     # SQLite pool (WAL, foreign keys) + migrations
│   ├── state.rs                  # AppState shared by handlers
│   ├── models/                   # Entities and their SQL queries
│   │   ├── monitor.rs            # Monitor configuration + CRUD
│   │   └── monitor_state.rs      # Runtime state written by the check engine
│   ├── forms/
│   │   └── monitor.rs            # MonitorForm (raw strings) → MonitorInput | FieldErrors
│   ├── views/                    # Askama template structs and view models
│   │   ├── errors.rs
│   │   └── monitors.rs
│   └── routes/                   # Handlers, grouped by domain
│       ├── mod.rs                # Router assembly + parse_id()
│       ├── health.rs             # GET /health
│       └── monitors.rs           # /, /monitors/*
├── templates/                    # Askama HTML templates
├── static/                       # CSS, fonts, images (embedded at build time)
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
- **Vocabulary.** The domain entity is a *monitor* everywhere: database, code, routes and UI.

## Configuration

| Variable       | Default                                 |
|----------------|-----------------------------------------|
| `DATABASE_URL` | `sqlite://uptime.db`                    |
| `BIND_ADDR`    | `0.0.0.0:8080`                          |
| `RUST_LOG`     | `uptime_monitoring=info,tower_http=info` |

An invalid value stops startup with a message.

## Tests

```bash
cargo test                                   # unit + integration tests (in-memory SQLite)
cargo clippy --all-targets -- -D warnings
```

Integration tests build the router on an in-memory database (`tests/common/mod.rs`) and call it with
`tower::ServiceExt::oneshot`; no server or network is involved.
