ALTER TABLE column_configs ADD COLUMN pane_index INTEGER;

-- Existing columns predate panes. Preserve the previous layout by assigning
-- each column to its own pane exactly once, as part of this migration.
UPDATE column_configs SET pane_index = position WHERE pane_index IS NULL;
