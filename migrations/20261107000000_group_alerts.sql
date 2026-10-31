-- Whether a channel gathers simultaneous downs (and ups) into one message. Webhooks never do.
ALTER TABLE notification_channels ADD COLUMN group_alerts INTEGER NOT NULL DEFAULT 1;
