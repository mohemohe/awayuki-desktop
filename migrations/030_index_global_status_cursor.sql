-- YQ/search walk statuses by this exact stable order. The previous
-- (created_at, id) index cannot satisfy the server_domain tie-break and made a
-- zero-result delta scan sort/scan the complete status cache.
CREATE INDEX IF NOT EXISTS idx_statuses_global_cursor
    ON statuses(created_at DESC, server_domain DESC, id DESC);
