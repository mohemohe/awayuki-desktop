ALTER TABLE column_configs
ADD COLUMN desktop_notifications INTEGER NOT NULL DEFAULT 1;

ALTER TABLE column_configs
ADD COLUMN notification_sound TEXT;
