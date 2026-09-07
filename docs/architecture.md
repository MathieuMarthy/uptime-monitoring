# Architecture

This document describes the project structure and conventions used in uptime-monitoring.

## Tech Stack

- **[Axum](https://github.com/tokio-rs/axum)** — HTTP framework
- **[Askama](https://github.com/djc/askama)** — Compile-time HTML templating (Jinja2-like syntax)
- **[askama_web](https://github.com/askama-rs/askama_web)** — Askama integration for Axum (implements `IntoResponse` for
  templates)
- **[SQLx](https://github.com/launchbadge/sqlx)** — Async SQL toolkit for SQLite with migrations and strong typing
- **[Tokio](https://tokio.rs/)** — Async runtime

## Project Structure

```
uptime-monitoring/
├── docs/                        # Project documentation
├── migrations/                  # SQL migration files
│   └── 202609061923_init.sql  # Schema definition (services, alerts, incidents, metrics)
├── src/
│   ├── main.rs                  # Entry point: DB init, builds router, starts server
│   ├── db.rs                    # SQLite pool initialization, WAL mode & auto-migrations
│   ├── state.rs                 # Shared AppState containing SqlitePool
│   ├── models/                  # Domain entities & database queries
│   │   ├── mod.rs
│   │   └── service.rs           # Service struct & queries
│   ├── routes/                  # Route handlers, grouped by domain
│   │   ├── mod.rs               # Re-exports modules + assembles all routes with AppState
│   │   ├── health.rs            # GET /health
│   │   └── monitors.rs          # /monitors routes
│   └── templates/               # Askama template structs (Rust side)
│       ├── mod.rs
│       └── monitors.rs          # MonitorIndexTemplate
└── templates/                   # Askama HTML templates (project root)
    ├── base.html                # Shared layout
    └── monitors/
        └── index.html           # Monitors dashboard page
```
