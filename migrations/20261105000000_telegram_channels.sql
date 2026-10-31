-- no-transaction
-- Telegram channels. `kind` has a CHECK constraint, so the table is rebuilt (SQLite's documented
-- procedure): foreign keys are off meanwhile, or dropping the old table would cascade to
-- monitor_channels and null notification_log.channel_id.
PRAGMA foreign_keys = OFF;

BEGIN;

CREATE TABLE notification_channels_new (
    id         INTEGER PRIMARY KEY,
    name       TEXT NOT NULL UNIQUE COLLATE NOCASE,
    kind       TEXT NOT NULL CHECK (kind IN ('webhook', 'discord', 'ntfy', 'telegram')),
    config     TEXT NOT NULL,
    enabled    INTEGER NOT NULL DEFAULT 1,
    created_at INTEGER NOT NULL
);

INSERT INTO notification_channels_new (id, name, kind, config, enabled, created_at)
SELECT id, name, kind, config, enabled, created_at
FROM notification_channels;

DROP TABLE notification_channels;
ALTER TABLE notification_channels_new RENAME TO notification_channels;

COMMIT;

PRAGMA foreign_keys = ON;
