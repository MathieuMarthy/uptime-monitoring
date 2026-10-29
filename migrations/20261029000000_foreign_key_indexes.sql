-- Deleting a channel, a monitor or a page looks up the rows that point to it: without these indexes, each delete
-- scans the whole child table.
CREATE INDEX idx_monitor_channels_channel ON monitor_channels(channel_id);
CREATE INDEX idx_notification_log_channel ON notification_log(channel_id);
CREATE INDEX idx_notification_log_monitor ON notification_log(monitor_id);
CREATE INDEX idx_status_page_monitors_monitor ON status_page_monitors(monitor_id);
