-- no-transaction
-- Push monitors: the monitored job calls /push/{push_token}; no ping within interval + grace is a
-- failure. `kind` has a CHECK constraint, so the table is rebuilt (SQLite's documented procedure):
-- foreign keys are off meanwhile, or dropping the old table would cascade to every child row.
PRAGMA foreign_keys = OFF;

BEGIN;

CREATE TABLE monitors_new (
    id                   INTEGER PRIMARY KEY,
    name                 TEXT NOT NULL,
    name_key             TEXT NOT NULL UNIQUE,
    kind                 TEXT NOT NULL DEFAULT 'http' CHECK (kind IN ('http', 'push')),
    -- Empty for a push monitor, which has no target.
    url                  TEXT NOT NULL,
    method               TEXT NOT NULL DEFAULT 'GET'
                         CHECK (method IN ('GET', 'HEAD', 'POST', 'PUT', 'PATCH', 'DELETE')),
    headers              TEXT,
    body                 TEXT,
    keyword              TEXT,
    accepted_statuses    TEXT NOT NULL DEFAULT '200-299',
    follow_redirects     INTEGER NOT NULL DEFAULT 1,
    ignore_tls_errors    INTEGER NOT NULL DEFAULT 0,
    interval_s           INTEGER NOT NULL DEFAULT 60,
    retry_interval_s     INTEGER NOT NULL DEFAULT 20,
    timeout_s            INTEGER NOT NULL DEFAULT 10,
    failure_threshold    INTEGER NOT NULL DEFAULT 3 CHECK (failure_threshold BETWEEN 1 AND 10),
    tls_expiry_warn_days INTEGER DEFAULT 14,
    paused               INTEGER NOT NULL DEFAULT 0,
    created_at           INTEGER NOT NULL,
    updated_at           INTEGER NOT NULL,
    group_name           TEXT,
    -- Secret of the push URL, kept readable so the page can show it again.
    push_token           TEXT UNIQUE,
    -- Push only: how late a ping may be before it counts as missed. 0 for HTTP.
    grace_s              INTEGER NOT NULL DEFAULT 0,
    CHECK ((kind = 'push') = (push_token IS NOT NULL)),
    CHECK ((kind = 'push') = (url = ''))
);

INSERT INTO monitors_new (
    id, name, name_key, kind, url, method, headers, body, keyword, accepted_statuses,
    follow_redirects, ignore_tls_errors, interval_s, retry_interval_s, timeout_s, failure_threshold,
    tls_expiry_warn_days, paused, created_at, updated_at, group_name
)
SELECT id, name, name_key, kind, url, method, headers, body, keyword, accepted_statuses,
       follow_redirects, ignore_tls_errors, interval_s, retry_interval_s, timeout_s,
       failure_threshold, tls_expiry_warn_days, paused, created_at, updated_at, group_name
FROM monitors;

DROP TABLE monitors;
ALTER TABLE monitors_new RENAME TO monitors;

COMMIT;

PRAGMA foreign_keys = ON;
