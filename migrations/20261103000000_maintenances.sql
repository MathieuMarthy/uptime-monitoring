-- Maintenance windows: while one is on, checks of its monitors run but count for nothing and
-- raise no alert. Either one-off (starts_at, ends_at) or weekly (weekdays, local_start,
-- duration_min, in the instance timezone).
CREATE TABLE maintenances (
    id                   INTEGER PRIMARY KEY,
    title                TEXT    NOT NULL,
    all_monitors         INTEGER NOT NULL DEFAULT 0,
    show_on_status_pages INTEGER NOT NULL DEFAULT 0,
    starts_at            INTEGER,
    ends_at              INTEGER,
    -- Bit 0 = Monday … bit 6 = Sunday.
    weekdays             INTEGER,
    -- Minutes after local midnight.
    local_start          INTEGER,
    duration_min         INTEGER,
    created_at           INTEGER NOT NULL,
    updated_at           INTEGER NOT NULL,
    CHECK (
        (starts_at IS NOT NULL AND ends_at > starts_at
            AND weekdays IS NULL AND local_start IS NULL AND duration_min IS NULL)
        OR (starts_at IS NULL AND ends_at IS NULL
            AND weekdays BETWEEN 1 AND 127
            AND local_start BETWEEN 0 AND 1439
            AND duration_min BETWEEN 1 AND 10080)
    )
);

CREATE TABLE maintenance_monitors (
    maintenance_id INTEGER NOT NULL REFERENCES maintenances(id) ON DELETE CASCADE,
    monitor_id     INTEGER NOT NULL REFERENCES monitors(id) ON DELETE CASCADE,
    PRIMARY KEY (maintenance_id, monitor_id)
);
CREATE INDEX maintenance_monitors_monitor ON maintenance_monitors(monitor_id);
