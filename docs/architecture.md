                                         # Architecture

This document describes the project structure and conventions used in uptime-monitoring.

## Tech Stack

- **[Axum](https://github.com/tokio-rs/axum)** — HTTP framework
- **[Askama](https://github.com/djc/askama)** — Compile-time HTML templating (Jinja2-like syntax)
- **[askama_web](https://github.com/askama-rs/askama_web)** — Askama integration for Axum (implements `IntoResponse` for
  templates)
- **[Tokio](https://tokio.rs/)** — Async runtime

## Project Structure

```
uptime-monitoring/
├── docs/                        # Project documentation
├── static/                      # Static assets (CSS, JS, images)
├── src/
│   ├── main.rs                  # Entry point: builds the router, starts the server
│   ├── routes/                  # Route handlers, grouped by domain
│   │   ├── mod.rs               # Re-exports modules + assembles all routes
│   │   ├── monitors.rs         # GET /
│   │   └── health.rs            # GET /health
│   └── templates/               # Askama template structs (Rust side)
│       ├── mod.rs               # Re-exports all template modules
│       ├── monitors.rs         # MonitorTemplate
└── templates/                   # Askama HTML templates (project root)
    └── monitors
        ├── index.html           # Monitor page
```
