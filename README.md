# uptime-monitoring

A lightweight, self-hosted uptime monitor: a single binary with SQLite, built to watch about a hundred HTTP(S)
endpoints from an instance exposed on the Internet, using as little memory as possible.

- HTTP(S) checks: method, headers, body, accepted status codes, keyword, redirects, timeouts.
- Down after *N* consecutive failures, faster re-checks while a failure is being confirmed.
- Alerts when a monitor goes down or comes back up, and before a TLS certificate expires: webhook, Discord, ntfy.
- Uptime computed from the actual downtime, hourly and daily bars, incidents, latency.
- Public status pages, each with its own slug and selection of monitors.
- One admin account: Argon2 password, session cookie, CSRF protection, login rate limit, strict CSP.
- Optional blocking of private addresses (localhost, private networks, cloud metadata), resistant to DNS
  rebinding.

About 3 MiB of RAM at rest; a 21 MB Docker image.

## Run it with Docker

```bash
docker run -d --name uptime-monitoring \
  -p 127.0.0.1:8080:8080 \
  -v uptime-data:/data \
  -e PUBLIC_URL=https://status.example.com \
  ghcr.io/mathieumarthy/uptime-monitoring:latest

docker logs uptime-monitoring   # prints the one-time setup URL
```

Open the setup URL from the logs to create the admin account. Until then, the instance only shows a page
pointing to the logs, so nobody else can claim it. A `compose.yaml` example is in the repository.

The session cookie is `Secure`: put the instance behind a reverse proxy that serves HTTPS (Caddy, Traefik,
nginx). Over plain HTTP, logging in only works on `localhost`.

## Configuration

| Variable              | Default                                  | Purpose |
|-----------------------|------------------------------------------|---------|
| `DATABASE_URL`        | `sqlite://uptime.db` (`/data/uptime.db` in the image) | SQLite file, created if missing |
| `BIND_ADDR`           | `0.0.0.0:8080`                           | Listening address |
| `PUBLIC_URL`          | unset                                    | Address users reach, e.g. `https://status.example.com`: alerts link to the monitor's page |
| `TRUST_PROXY_HEADERS` | `false`                                  | Take the client IP from `X-Forwarded-For`; enable only behind a proxy |
| `RUST_LOG`            | `uptime_monitoring=info,tower_http=info` | Log filter |

Timezone, data retention, private-address blocking and the password are changed from the *Settings* page.

## Build from source

```bash
cargo build --release          # target/release/uptime-monitoring, assets embedded
cargo test
cargo clippy --all-targets -- -D warnings
```

The binary is self-contained and runs from any directory. TLS certificates are checked against the system
store.

Design notes: [`docs/architecture.md`](docs/architecture.md), [`docs/database-schema.md`](docs/database-schema.md).
