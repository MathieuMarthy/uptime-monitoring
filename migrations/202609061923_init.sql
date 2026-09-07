CREATE TABLE IF NOT EXISTS services (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    url TEXT NOT NULL,
    method TEXT NOT NULL DEFAULT 'GET',
    interval_seconds INTEGER NOT NULL DEFAULT 60,
    timeout_seconds INTEGER NOT NULL DEFAULT 10,
    expected_status INTEGER NOT NULL DEFAULT 200,
    headers TEXT,
    body TEXT,
    keyword_match TEXT,
    max_retries INTEGER NOT NULL DEFAULT 1,
    is_enabled INTEGER NOT NULL DEFAULT 1,
    current_status TEXT NOT NULL DEFAULT 'up' CHECK(current_status IN ('up', 'down', 'pending')),
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS alerts (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    provider TEXT NOT NULL,
    config TEXT NOT NULL,
    is_enabled INTEGER NOT NULL DEFAULT 1,
    notify_on_resolved INTEGER NOT NULL DEFAULT 1
);

CREATE TABLE IF NOT EXISTS service_alerts (
    service_id TEXT NOT NULL REFERENCES services(id) ON DELETE CASCADE,
    alert_id TEXT NOT NULL REFERENCES alerts(id) ON DELETE CASCADE,
    PRIMARY KEY (service_id, alert_id)
);

CREATE TABLE IF NOT EXISTS incidents (
    id TEXT PRIMARY KEY,
    service_id TEXT NOT NULL REFERENCES services(id) ON DELETE CASCADE,
    started_at INTEGER NOT NULL,
    resolved_at INTEGER,
    cause TEXT,
    error_type TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_incidents_service ON incidents(service_id, started_at DESC);

CREATE TABLE IF NOT EXISTS metrics (
    service_id TEXT NOT NULL REFERENCES services(id) ON DELETE CASCADE,
    timestamp INTEGER NOT NULL,
    response_time_ms INTEGER NOT NULL,
    status_code INTEGER,
    PRIMARY KEY (service_id, timestamp)
);

CREATE INDEX IF NOT EXISTS idx_metrics_service_time ON metrics(service_id, timestamp DESC);
