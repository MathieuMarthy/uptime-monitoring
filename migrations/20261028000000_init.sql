-- Timestamps are Unix epoch milliseconds. Booleans are 0/1.

CREATE TABLE monitors (
    id                   INTEGER PRIMARY KEY,
    name                 TEXT NOT NULL UNIQUE COLLATE NOCASE,
    kind                 TEXT NOT NULL DEFAULT 'http' CHECK (kind IN ('http')),
    url                  TEXT NOT NULL,
    method               TEXT NOT NULL DEFAULT 'GET'
                         CHECK (method IN ('GET', 'HEAD', 'POST', 'PUT', 'PATCH', 'DELETE')),
    headers              TEXT,
    body                 TEXT,
    keyword              TEXT,
    accepted_statuses    TEXT NOT NULL DEFAULT '200-299',
    follow_redirects     INTEGER NOT NULL DEFAULT 1,
    interval_s           INTEGER NOT NULL DEFAULT 60,
    timeout_s            INTEGER NOT NULL DEFAULT 10,
    failure_threshold    INTEGER NOT NULL DEFAULT 1 CHECK (failure_threshold BETWEEN 1 AND 10),
    tls_expiry_warn_days INTEGER DEFAULT 14,
    paused               INTEGER NOT NULL DEFAULT 0,
    created_at           INTEGER NOT NULL,
    updated_at           INTEGER NOT NULL
);

-- Runtime state, written only by the check engine.
CREATE TABLE monitor_state (
    monitor_id           INTEGER PRIMARY KEY REFERENCES monitors(id) ON DELETE CASCADE,
    status               TEXT NOT NULL DEFAULT 'pending' CHECK (status IN ('pending', 'up', 'down')),
    consecutive_failures INTEGER NOT NULL DEFAULT 0,
    last_check_at        INTEGER,
    last_latency_ms      INTEGER,
    last_error           TEXT,
    tls_expires_at       INTEGER,
    tls_warned_for       INTEGER
);

-- Raw check results, short retention.
CREATE TABLE checks (
    monitor_id  INTEGER NOT NULL REFERENCES monitors(id) ON DELETE CASCADE,
    checked_at  INTEGER NOT NULL,
    ok          INTEGER NOT NULL,
    latency_ms  INTEGER,
    status_code INTEGER,
    error_kind  TEXT,
    error       TEXT,
    PRIMARY KEY (monitor_id, checked_at)
) WITHOUT ROWID;

-- Outage periods, long retention.
CREATE TABLE incidents (
    id          INTEGER PRIMARY KEY,
    monitor_id  INTEGER NOT NULL REFERENCES monitors(id) ON DELETE CASCADE,
    started_at  INTEGER NOT NULL,
    resolved_at INTEGER,
    error_kind  TEXT NOT NULL,
    cause       TEXT
);
CREATE INDEX idx_incidents_monitor ON incidents(monitor_id, started_at DESC);
CREATE UNIQUE INDEX idx_incidents_open ON incidents(monitor_id) WHERE resolved_at IS NULL;

-- Daily rollups, long retention. `day` is the instance-local date as YYYYMMDD.
CREATE TABLE daily_stats (
    monitor_id     INTEGER NOT NULL REFERENCES monitors(id) ON DELETE CASCADE,
    day            INTEGER NOT NULL,
    monitored_ms   INTEGER NOT NULL,
    downtime_ms    INTEGER NOT NULL,
    checks_total   INTEGER NOT NULL,
    checks_failed  INTEGER NOT NULL,
    latency_avg_ms INTEGER,
    latency_p95_ms INTEGER,
    PRIMARY KEY (monitor_id, day)
) WITHOUT ROWID;

CREATE TABLE notification_channels (
    id         INTEGER PRIMARY KEY,
    name       TEXT NOT NULL UNIQUE COLLATE NOCASE,
    kind       TEXT NOT NULL CHECK (kind IN ('webhook', 'discord', 'ntfy')),
    config     TEXT NOT NULL,
    enabled    INTEGER NOT NULL DEFAULT 1,
    created_at INTEGER NOT NULL
);

CREATE TABLE monitor_channels (
    monitor_id INTEGER NOT NULL REFERENCES monitors(id) ON DELETE CASCADE,
    channel_id INTEGER NOT NULL REFERENCES notification_channels(id) ON DELETE CASCADE,
    PRIMARY KEY (monitor_id, channel_id)
) WITHOUT ROWID;

CREATE TABLE notification_log (
    id         INTEGER PRIMARY KEY,
    channel_id INTEGER REFERENCES notification_channels(id) ON DELETE SET NULL,
    monitor_id INTEGER REFERENCES monitors(id) ON DELETE SET NULL,
    event      TEXT NOT NULL CHECK (event IN ('down', 'up', 'tls_expiring', 'test')),
    sent_at    INTEGER NOT NULL,
    ok         INTEGER NOT NULL,
    error      TEXT
);

CREATE TABLE status_pages (
    id          INTEGER PRIMARY KEY,
    slug        TEXT NOT NULL UNIQUE,
    title       TEXT NOT NULL,
    description TEXT,
    published   INTEGER NOT NULL DEFAULT 0,
    created_at  INTEGER NOT NULL,
    updated_at  INTEGER NOT NULL
);

CREATE TABLE status_page_monitors (
    page_id      INTEGER NOT NULL REFERENCES status_pages(id) ON DELETE CASCADE,
    monitor_id   INTEGER NOT NULL REFERENCES monitors(id) ON DELETE CASCADE,
    display_name TEXT,
    position     INTEGER NOT NULL,
    PRIMARY KEY (page_id, monitor_id)
) WITHOUT ROWID;

CREATE TABLE admin (
    id            INTEGER PRIMARY KEY CHECK (id = 1),
    username      TEXT NOT NULL,
    password_hash TEXT NOT NULL,
    updated_at    INTEGER NOT NULL
);

-- Only the SHA-256 of a session token is stored.
CREATE TABLE sessions (
    token_hash TEXT PRIMARY KEY,
    csrf_token TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL
) WITHOUT ROWID;

CREATE TABLE settings (
    id                     INTEGER PRIMARY KEY CHECK (id = 1),
    timezone               TEXT NOT NULL DEFAULT 'UTC',
    checks_retention_days  INTEGER NOT NULL DEFAULT 7,
    history_retention_days INTEGER,
    block_private_targets  INTEGER NOT NULL DEFAULT 0
);

INSERT INTO settings (id) VALUES (1);
