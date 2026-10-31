-- no-transaction
-- Degraded state: a latency threshold per monitor, the slow-check series in monitor_state, and
-- the slowdowns, kept like incidents. `notification_log.event` has a CHECK constraint, so that
-- table is rebuilt (SQLite's documented procedure); nothing references it.
PRAGMA foreign_keys = OFF;

BEGIN;

-- NULL: no threshold. Always NULL for push monitors.
ALTER TABLE monitors ADD COLUMN latency_warn_ms INTEGER CHECK (latency_warn_ms > 0);
ALTER TABLE monitors ADD COLUMN latency_alerts INTEGER NOT NULL DEFAULT 1;

ALTER TABLE monitor_state ADD COLUMN slow_checks INTEGER NOT NULL DEFAULT 0;
ALTER TABLE monitor_state ADD COLUMN slow_since INTEGER;
ALTER TABLE monitor_state ADD COLUMN degraded_since INTEGER;

CREATE TABLE slowdowns (
    id           INTEGER PRIMARY KEY,
    monitor_id   INTEGER NOT NULL REFERENCES monitors(id) ON DELETE CASCADE,
    started_at   INTEGER NOT NULL,
    resolved_at  INTEGER,
    -- The threshold in force when it started.
    threshold_ms INTEGER NOT NULL,
    -- Its start was alerted: its end is alerted too, whatever the setting by then.
    alerted      INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX idx_slowdowns_monitor ON slowdowns(monitor_id, started_at DESC);
CREATE INDEX idx_slowdowns_started ON slowdowns(started_at);
CREATE UNIQUE INDEX idx_slowdowns_open ON slowdowns(monitor_id) WHERE resolved_at IS NULL;

CREATE TABLE notification_log_new (
    id         INTEGER PRIMARY KEY,
    channel_id INTEGER REFERENCES notification_channels(id) ON DELETE SET NULL,
    monitor_id INTEGER REFERENCES monitors(id) ON DELETE SET NULL,
    event      TEXT NOT NULL
               CHECK (event IN ('down', 'up', 'tls_expiring', 'degraded', 'latency_normal', 'test')),
    sent_at    INTEGER NOT NULL,
    ok         INTEGER NOT NULL,
    error      TEXT
);

INSERT INTO notification_log_new (id, channel_id, monitor_id, event, sent_at, ok, error)
SELECT id, channel_id, monitor_id, event, sent_at, ok, error
FROM notification_log;

DROP TABLE notification_log;
ALTER TABLE notification_log_new RENAME TO notification_log;
CREATE INDEX idx_notification_log_channel ON notification_log(channel_id);
CREATE INDEX idx_notification_log_monitor ON notification_log(monitor_id, sent_at);

COMMIT;

PRAGMA foreign_keys = ON;
