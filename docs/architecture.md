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
│   ├── windows.rs                # Maintenance windows in time (pure) + Maintenances, the shared set
│   ├── maintenance.rs            # Hourly upkeep: daily rollups, then purges
│   ├── page_cache.rs             # Rendered public pages, kept 60 s
│   ├── assets.rs                 # /static/* served from embedded files
│   ├── db.rs                     # SQLite pool (WAL, foreign keys) + migrations
│   ├── state.rs                  # AppState: pool, setup token, login limiter, hashing semaphore, engine handle,
│   │                             # guard, notifier, public page cache, PUBLIC_URL, form-test semaphore,
│   │                             # maintenance windows
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
│   │   ├── maintenance.rs        # Maintenance windows + their monitors
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
│   │   ├── incident.rs           # NoteForm → NoteInput
│   │   ├── maintenance.rs        # MaintenanceForm → MaintenanceInput | FieldErrors
│   │   ├── monitor.rs            # MonitorForm (raw strings) → MonitorInput | FieldErrors
│   │   ├── settings.rs           # GeneralForm, DataForm, PasswordForm
│   │   └── status_page.rs        # StatusPageForm → StatusPageInput | FieldErrors
│   ├── views/                    # Askama template structs and view models
│   │   ├── activity.rs           # Activity page: events of all monitors, filters, pages
│   │   ├── alerts.rs
│   │   ├── auth.rs
│   │   ├── errors.rs
│   │   ├── heartbeat.rs          # Hourly and daily bars, their axis, uptime labels
│   │   ├── incidents.rs          # An incident unfolded: facts, failed checks, alerts sent
│   │   ├── maintenances.rs       # Maintenance list and form
│   │   ├── monitors.rs
│   │   ├── settings.rs           # Settings page, disk estimates
│   │   └── status_pages.rs       # Admin list and form, public page
│   └── routes/                   # Handlers, grouped by domain
│       ├── mod.rs                # Router assembly (public / protected) + parse_id()
│       ├── activity.rs           # /activity
│       ├── alerts.rs             # /alerts/*
│       ├── auth.rs               # /setup, /login, /logout
│       ├── health.rs             # GET /health
│       ├── incidents.rs          # /incidents/{id}/note
│       ├── maintenances.rs       # /maintenances/*, /monitors/{id}/maintenance
│       ├── monitors.rs           # /, /monitors/*
│       ├── settings.rs           # /settings/*
│       └── status_pages.rs       # /public-pages/* (admin), /status/{slug} (public)
├── templates/                    # Askama templates: base.html → layout.html (admin) | public.html;
│                                 # partials/ holds fragments included by several pages
├── static/                       # CSS, JS, fonts (embedded at build time)
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
  under its input instead of a raw 422. Validation reports all errors at once. Lengths are counted in UTF-16 units (`forms::text_len`), as
  the browser's `maxlength` counts them.
- **Ids.** Integer ids. Path ids are parsed with `routes::parse_id` (the `PathId` extractor); anything that is not a positive integer is a 404.
- **Time.** Timestamps are Unix epoch milliseconds (`i64`), from `time::now_ms()`.
- **Protected by default.** `routes::all` merges a protected router, behind `middleware::require_session`, and a
  public one (`/health`, `/setup`, `/login`, `/status/{slug}`, `/push/{token}`, static assets). A new route goes in the protected router unless it must be
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
| `PUID`, `PGID` | `65532` (only used when started as root) |

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
- **Push monitors.** `kind = 'push'`: no request goes out. `GET|HEAD|POST /push/{token}` looks up the active monitor
  (unknown token → 404, paused → 200 and ignored) and hands a success, or with `?status=down&msg=…` a `reported`
  failure, to `EngineHandle::ping`. Pings go through a bounded queue (256) to the supervisor, then to the monitor's
  task (4); beyond, they are dropped, as are those queued when a reload restarts the task. The task
  (`Scheduler::run_push`) records a ping at most once per second; a ping ready at a deadline wins over it. With no
  ping `interval_s + grace_s` after the last check, it records a `missed` failure, then one more per `interval_s` (a
  push monitor stores `retry_interval_s = interval_s`: `failure_threshold` counts missed runs). Deadlines count from
  the last recorded check, so a restart or an edit does not postpone them; an overdue monitor gets `grace_s` after the
  task starts. Recording, incidents and alerts are shared with HTTP checks; alerts carry no URL (`null` in webhooks).
- **Form test.** The monitor form's Test button posts to `/monitors/new/test` or `/monitors/{id}/edit/test`: the
  values are validated, then checked once by `AppState::form_checker` (its own `HttpChecker`, with the shared
  `Guard`), with the timeout capped at 30 s, and the form comes back with the result; nothing is saved. A certificate
  already within the warning period is pointed out: saving would alert at once. Two tests run at once at most
  (`AppState::monitor_tests`); a third is told to retry. Push monitors have nothing to test.
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
  A pause resolves the open incident at the last check: nothing is known after it.
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

## Maintenance windows

- **Schedule** (`windows.rs`, pure). One-off (`[starts_at, ends_at)`) or weekly: days (bit 0 = Monday), a local
  start time in the instance timezone and a duration in real time, at most the gap between two picked days (a week for
  one day) so occurrences never overlap. A start in a DST gap moves forward
  by the gap (jiff's compatible rule); in a fold, the first is taken.
- **Shared set.** `Maintenances` holds every window in memory (`Arc<RwLock>`), with the timezone. Saving, deleting or
  starting a window, and changing the timezone, reload it whole, and drop the cached status pages; deleting a monitor
  reloads it too, since SQLite reuses the last id. Reloads run one at a time. The engine asks it at each check.
- **Effect on a check** (`MonitorState::record`, `in_maintenance`). The check is recorded and shown, with
  `covered_ms = 0` and `down = 0`, so it counts in no uptime figure. The state machine is frozen (`verdict::frozen`):
  no transition, no incident opened or resolved, no alert, no certificate warning; a failure series below the
  threshold starts again after the window. A success ends an outage: the incident is resolved at that check, without
  an *up* alert; a failure leaves a down monitor down, its incident open. The check after the window covers the gap since the last one as usual. A push
  monitor's missed deadline in a window is a check like the others.
- **Pages.** `/maintenances` lists windows in progress, upcoming and past, and edits them; the form's group boxes
  pick the group's members when saved (no lasting link to the group). A monitor's page has *Maintenance now* (15 min
  to 4 h, one-off window on that monitor). Dashboard, detail and status pages show *Maintenance* (half-filled blue
  lamp; after *Paused*, before the state); such a monitor is not counted down in headlines. A status page shows a
  banner per window in progress on its monitors, titled only when the window is announced
  (`show_on_status_pages`), and announced windows starting within 7 days. The Activity page computes starts and
  ends from the schedules (`type=maintenance`), from the window's `created_at` on (a window created while on starts
  then); a deleted or edited window rewrites its past events.
- **Retention.** One-off windows ended before the history cutoff are purged with the history; weekly ones stay.

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
- **Dashboard.** One query for monitors and states (no body or headers), one for the 24 hourly bars of all monitors
  (`?range=90d`: the 90 daily bars, through `DailyHistory::load_many`). Monitors are listed down first, then pending,
  up and paused, by name within each state; a sentence on top names the down ones. Every row shares one time axis.
- **Groups.** A monitor's optional `group_name` is free text, invisible characters dropped and spaces collapsed so
  that look-alike names make one group (*Ungrouped* is refused: it titles the monitors without one); its form suggests the groups in use (`<datalist>`). A group
  exists while a monitor uses it: renaming means editing its monitors. Once any monitor has a group, the dashboard
  shows one `<details>` per group, by name ignoring case (spelled as on its first monitor by name), *Ungrouped* last,
  each headed by its worst state's lamp and its counts; the order above applies within a group, the headline still
  covers every monitor. The `<details>` id is the hex of the group's key, so `live.js` keeps it folded across
  refreshes. Without any group, the list has no headers.
  Bars (`Tick::class`): a dot without checks, short green, mid-height orange with failures but no downtime, red with
  downtime, full height once it reaches a tenth of the bar's time. Uptime is truncated, never rounded up to 100 %.
- **Detail page.** A status sentence (from `monitor_state` and the latest incident), 24 h and 30 d uptime, 24 h average
  latency, days left on the certificate, 24 hourly bars with the hourly average latency drawn as bars under
  them, 90 daily bars (daily stats, then raw checks for the days not rolled up yet), the last 20 incidents unfolded
  like on the Activity page, the last 20 checks, then the configuration.
- **Live refresh.** `static/js/live.js` refetches the page every 30 s while the tab is visible and swaps the elements
  marked `data-live` (each with an id) whose content changed, keeping the focused link or summary and the `<details>`
  the user opened or closed (by id). A changed status sentence (`data-announce`) is read out by a polite live region,
  nothing else. A redirect (expired session) reloads the page. No extra route or JSON. A form being edited
  (focused, or changed since load, such as an incident's note) is carried over into the swapped block by id.
- **Incident notes.** Each unfolded incident (detail and Activity pages) has a note form posting to
  `/incidents/{id}/note`. Line breaks count once, as `maxlength` counts them. Saving drops every cached status page,
  so a public note shows (or goes) at once, then returns to the `Referer`'s path and query (never its host) at
  `#incident-{id}`, which `open-target.js` unfolds; the monitor's page without a `Referer`.
- **Search and filters.** The dashboard state lives in its URL (`/?range=90d&q=api&status=down&group=Web`):
  `DashboardFilter` renders every row and marks those left out `hidden`, with the groups left empty, so the page
  works without JS and a link shares it. The search matches name, URL and group (`data-search`), ignoring case and
  accents; `status` is `down` or `paused`; `group` compares like group names, `Ungrouped` picking the monitors
  without one. The state chips count every monitor and are a `data-live` block. `static/js/filter.js` applies the
  same filter at once in the browser, keeps the URL in step with `history.replaceState` (so the live refresh
  fetches the same filter), unfolds groups with matches; `/` focuses the search, Esc clears it.
- **Bulk actions.** The dashboard rows sit in one form (`POST /monitors/bulk`: `action`, `id` repeated, `group` or
  `channel`, `back`): pause, resume, move to a group (cleaned by `clean_group`, as in the monitor form), link or
  unlink a channel, delete. Each is one statement over `json_each`; unknown ids are skipped and the count is what
  actually changed. The redirect goes back to `back` (only `/` or `/?…`, never off the site) with
  `done=paused&count=2`, shown as a notice (an error banner when nothing was done); `bulk.js` then drops it from
  the URL, so a reload does not repeat it. `static/js/bulk.js` shows the actions once a row is ticked, ticks the
  rows on screen on "Select all", and unticks a row the filter hides or a folded group covers. The form is in the live block, so `live.js` keeps it, rows frozen,
  while a box is ticked. The Move button comes first in the form so Enter in the group field moves.

## Activity

`GET /activity` lists, latest first and grouped by local day, the incidents started in the period, the failed checks
that did not count as downtime (`ok = 0 AND NOT down`, minus the first failure of each incident), the last
certificate warning of each monitor (`monitor_state.tls_warned_at`) and the starts and ends of maintenance windows.
Failed checks made during a window show too: they did not count. Filters are query parameters read leniently
(anything unexpected falls back to the default): `type` (`outages`, `checks`, `certificates`, `maintenance`), `monitor`, `days` (7,
30 or 90) and `before`, the cursor of the next page: `time-source-id`, since events sort by time, then source, then
id, so two events logged in the same millisecond never fall between pages. A page holds 50 events: each source is
read with `LIMIT 51`, then merged. An incident unfolds (`<details>`, no JS) to its facts, its last 10 failed checks
with their count (one query, `COUNT(*) OVER ()`) and the alerts logged for it: its down alert before the next incident
of the monitor starts, its up alert within 5 min of its end and before the next incident ends. Failed checks and
alerts follow the checks retention: older incidents only show their facts.

## Look

- **Theme.** One stylesheet of tokens (`static/css/style.css`): an enamel grey-green ground, plates for panels, round
  status lamps (hollow when paused). Each colour is `light-dark(day, night)`: the system theme picks one, unless the
  header's switch (system, light, dark) set `data-theme` on `<html>`. `static/js/theme.js`, loaded without `defer`
  in `<head>`, applies the choice before the page is drawn; it is kept in `localStorage`, so per browser. The status
  page has no switch but follows a choice made in the admin.
- **Fonts.** Barlow (text) and Barlow Semi Condensed (titles), self-hosted woff2 under the SIL Open Font License
  (`static/fonts/OFL.txt`), tabular figures everywhere.
- **Traces.** The height of a bar tells the state, its colour only confirms it, so the bars read without colour.
  Their tooltip is CSS (`data-tip`), a mouse convenience: the detail page lists the same figures in an *Hourly
  figures* table, and dashboard rows hide the trace from screen readers (the row reads name, state, uptime and
  response). A computed height is never a `style` attribute, which the CSP forbids: the latency bars take one of 20
  height classes (`lat-1` to `lat-20`).
- **Motion.** Bars rise once on load (`live.js` then marks the page `settled`, so swapped blocks stay still), a down
  lamp sends out a ring three times (under the 5 s after which moving content must be pausable), tooltips and
  unfolded incidents fade in. `prefers-reduced-motion` turns all of it off.
- **Accessibility.** Colour pairs meet WCAG AA in both palettes (4.5:1 for text, 3:1 for lamps and bars); a state is
  always written out next to its lamp, except *Up* (visually hidden).
- **Menu.** A page marks its menu entry by filling the matching `nav_*` block of `layout.html`. Under 720 px, CSS alone
  puts the brand and three icon buttons (theme, settings, log out; named by `aria-label`) on one line and
  the five sections as equal tabs under them (*Maintenance* shortened to *Maint.*).

## Public pages

- **Admin** (`/public-pages`). A page has a title, a slug (`[a-z0-9]+(-[a-z0-9]+)*`, at most 64, unique), an
  optional description and a *published* flag. Its form lists every monitor: checkbox `monitor_<id>`, display name
  `display_name_<id>` (blank = the monitor's name) and order `position_<id>` (0–9999; blank orders last, ties keep
  the form's order, by name). Stored positions are renumbered from 0. `/public-pages/{id}/preview` shows the page,
  published or not, never cached.
- **Public** (`GET /status/{slug}`, no session). An unpublished page is a 404, like an unknown slug. It shows the
  overall state as a sentence naming what is down (*Billing is down*, *2 services are down*, *All services are
  running*; paused monitors are ignored), then per monitor its display
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
  `gcr.io/distroless/static-debian12`, which has the CA certificates and no shell. `DATABASE_URL` points to the
  `/data` volume. About 21 MB, under 3 MiB of RAM at rest. amd64 only.
- **Privileges** (`privileges.rs`). The image starts as root, like official images whose entrypoint runs `chown`
  then `gosu`, but without a shell: before the async runtime, any file or socket, the binary gives the database's
  directory, the database and its journals (nothing else) to `PUID:PGID` (default 65532), without following
  symlinks, then drops supplementary groups, sets its gid and uid, and checks that root cannot be regained. A
  bind-mounted directory created by Docker as root thus works as is. Started as another user (`user:` in Compose,
  Kubernetes), it changes nothing. The `healthcheck` subcommand drops root too, without touching files.
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
