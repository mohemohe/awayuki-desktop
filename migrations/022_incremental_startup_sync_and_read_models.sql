-- Persist startup synchronization at the account/phase boundary. A page cursor
-- is committed only after the page data, allowing an interrupted full scan to
-- resume without replaying already-persisted pages.
-- OAuth app registration credentials were never read after being written.
-- Keeping that dead secret surface contradicts the SQLite-only minimal-state
-- contract, so remove it before creating the new maintenance metadata.
DROP TABLE IF EXISTS client_credentials;

CREATE TABLE startup_sync_state (
    account_acct TEXT NOT NULL,
    phase TEXT NOT NULL,
    high_water_id TEXT,
    resume_cursor TEXT,
    last_success_at TEXT,
    last_full_reconcile_at TEXT,
    reconciliation_generation TEXT,
    api_requests INTEGER NOT NULL DEFAULT 0,
    db_writes INTEGER NOT NULL DEFAULT 0,
    last_duration_ms INTEGER NOT NULL DEFAULT 0,
    last_error TEXT,
    PRIMARY KEY (account_acct, phase),
    FOREIGN KEY (account_acct)
        REFERENCES login_accounts(acct) ON DELETE CASCADE
);

CREATE INDEX idx_startup_sync_due
    ON startup_sync_state(phase, last_success_at, last_full_reconcile_at);

-- Full bookmark/favourite reconciliation is staged by generation. Staging is
-- intentionally durable: a crash keeps both the cursor and the seen set.
CREATE TABLE startup_sync_reconciliation_members (
    account_acct TEXT NOT NULL,
    phase TEXT NOT NULL,
    generation TEXT NOT NULL,
    status_id TEXT NOT NULL,
    server_domain TEXT NOT NULL,
    PRIMARY KEY (account_acct, phase, generation, status_id, server_domain),
    FOREIGN KEY (account_acct)
        REFERENCES login_accounts(acct) ON DELETE CASCADE,
    FOREIGN KEY (status_id, server_domain)
        REFERENCES statuses(id, server_domain) ON DELETE CASCADE
);

-- Explicitly protect cached conversation rows that were opened as a thread.
-- Bookmarks, favourites, notifications and timeline membership are protected
-- by their own normalized relations and do not need duplicate rows here.
CREATE TABLE status_retention_protections (
    status_id TEXT NOT NULL,
    server_domain TEXT NOT NULL,
    reason TEXT NOT NULL,
    last_accessed_at TEXT NOT NULL,
    PRIMARY KEY (status_id, server_domain, reason),
    FOREIGN KEY (status_id, server_domain)
        REFERENCES statuses(id, server_domain) ON DELETE CASCADE
);

CREATE INDEX idx_status_retention_protections_accessed
    ON status_retention_protections(last_accessed_at);

-- Frequently-polled totals are maintained at write time instead of rescanning
-- the complete cache every 15 seconds.
CREATE TABLE cache_counters (
    name TEXT PRIMARY KEY,
    value INTEGER NOT NULL CHECK (value >= 0),
    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);

INSERT INTO cache_counters(name, value)
VALUES ('statuses', (SELECT COUNT(*) FROM statuses));
INSERT INTO cache_counters(name, value)
VALUES ('accounts', (SELECT COUNT(*) FROM accounts));

CREATE TRIGGER cache_counter_status_insert
AFTER INSERT ON statuses
BEGIN
    UPDATE cache_counters
       SET value = value + 1, updated_at = datetime('now')
     WHERE name = 'statuses';
END;

CREATE TRIGGER cache_counter_status_delete
AFTER DELETE ON statuses
BEGIN
    UPDATE cache_counters
       SET value = MAX(0, value - 1), updated_at = datetime('now')
     WHERE name = 'statuses';
END;

CREATE TRIGGER cache_counter_account_insert
AFTER INSERT ON accounts
BEGIN
    UPDATE cache_counters
       SET value = value + 1, updated_at = datetime('now')
     WHERE name = 'accounts';
END;

CREATE TRIGGER cache_counter_account_delete
AFTER DELETE ON accounts
BEGIN
    UPDATE cache_counters
       SET value = MAX(0, value - 1), updated_at = datetime('now')
     WHERE name = 'accounts';
END;

-- Cover the bounded candidate page before canonical de-duplication. The old
-- lookup index has account_acct between timeline_type and position_at, which
-- prevents a global aggregate from reading newest-first across accounts.
CREATE INDEX idx_timeline_entries_aggregate_page
    ON timeline_entries(
        timeline_type,
        position_at DESC,
        server_domain DESC,
        status_id DESC,
        account_acct DESC
    );

CREATE INDEX idx_status_identities_canonical_cover
    ON status_identities(canonical_uri, server_domain, status_id);

CREATE INDEX idx_statuses_reply_tree
    ON statuses(server_domain, in_reply_to_id, created_at, id)
    WHERE in_reply_to_id IS NOT NULL;

CREATE INDEX idx_notifications_global_page
    ON notifications(created_at DESC, server_domain DESC, id DESC, account_acct DESC);
