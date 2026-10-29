-- The Activity page: when the last certificate warning was given, and indexes to list events by
-- time across monitors.
ALTER TABLE monitor_state ADD COLUMN tls_warned_at INTEGER;

-- Failures that were not downtime are few: the page finds them without reading every check, nor
-- the failures of a long outage.
CREATE INDEX idx_checks_failed ON checks(checked_at) WHERE ok = 0 AND down = 0;
CREATE INDEX idx_incidents_started ON incidents(started_at);

-- The alerts of one monitor around an incident.
DROP INDEX idx_notification_log_monitor;
CREATE INDEX idx_notification_log_monitor ON notification_log(monitor_id, sent_at);
