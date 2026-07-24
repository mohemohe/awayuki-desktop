-- Canonical content remains in statuses.  Protocol identity and values that
-- depend on the viewing login account are normalized into separate tables.
CREATE TABLE status_identities (
    status_id TEXT NOT NULL,
    server_domain TEXT NOT NULL,
    protocol TEXT NOT NULL CHECK (protocol IN ('activitypub', 'atproto')),
    canonical_uri TEXT NOT NULL,
    remote_id TEXT NOT NULL,
    PRIMARY KEY (status_id, server_domain),
    UNIQUE (protocol, server_domain, remote_id),
    FOREIGN KEY (status_id, server_domain)
        REFERENCES statuses(id, server_domain) ON DELETE CASCADE
);

CREATE INDEX idx_status_identities_canonical_uri
    ON status_identities(protocol, canonical_uri);

INSERT INTO status_identities (
    status_id, server_domain, protocol, canonical_uri, remote_id
)
SELECT
    s.id,
    s.server_domain,
    CASE WHEN s.uri LIKE 'at://%' THEN 'atproto' ELSE 'activitypub' END,
    s.uri,
    s.id
FROM statuses s;

CREATE TABLE status_viewer_state (
    login_account_acct TEXT NOT NULL,
    status_id TEXT NOT NULL,
    server_domain TEXT NOT NULL,
    favourited INTEGER,
    reblogged INTEGER,
    muted INTEGER,
    bookmarked INTEGER,
    pinned INTEGER,
    updated_at TEXT NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (login_account_acct, status_id, server_domain),
    FOREIGN KEY (login_account_acct)
        REFERENCES login_accounts(acct) ON DELETE CASCADE,
    FOREIGN KEY (status_id, server_domain)
        REFERENCES statuses(id, server_domain) ON DELETE CASCADE
);

CREATE INDEX idx_status_viewer_state_favourites
    ON status_viewer_state(login_account_acct, updated_at DESC)
    WHERE favourited = 1;
CREATE INDEX idx_status_viewer_state_bookmarks
    ON status_viewer_state(login_account_acct, updated_at DESC)
    WHERE bookmarked = 1;

-- Timeline membership is the strongest evidence of which login account
-- observed the legacy viewer flags.
INSERT OR IGNORE INTO status_viewer_state (
    login_account_acct, status_id, server_domain,
    favourited, reblogged, muted, bookmarked, pinned, updated_at
)
SELECT DISTINCT
    te.account_acct, s.id, s.server_domain,
    s.favourited, s.reblogged, s.muted, s.bookmarked, s.pinned, s.fetched_at
FROM timeline_entries te
JOIN login_accounts la ON la.acct = te.account_acct
JOIN statuses s ON s.id = te.status_id AND s.server_domain = te.server_domain;

-- Notifications also identify the receiving account, including statuses that
-- were never inserted into an ordinary timeline.
INSERT OR IGNORE INTO status_viewer_state (
    login_account_acct, status_id, server_domain,
    favourited, reblogged, muted, bookmarked, pinned, updated_at
)
SELECT DISTINCT
    n.account_acct, s.id, s.server_domain,
    s.favourited, s.reblogged, s.muted, s.bookmarked, s.pinned, s.fetched_at
FROM notifications n
JOIN login_accounts la ON la.acct = n.account_acct
JOIN statuses s ON s.id = n.status_id AND s.server_domain = n.server_domain
WHERE n.account_acct IS NOT NULL;

-- Old rows did not record the viewer. Replicating their one observable value
-- to same-server login accounts is lossless; later syncs replace each scoped
-- row independently.
INSERT OR IGNORE INTO status_viewer_state (
    login_account_acct, status_id, server_domain,
    favourited, reblogged, muted, bookmarked, pinned, updated_at
)
SELECT
    la.acct, s.id, s.server_domain,
    s.favourited, s.reblogged, s.muted, s.bookmarked, s.pinned, s.fetched_at
FROM statuses s
JOIN login_accounts la ON la.server_domain = s.server_domain
WHERE s.favourited IS NOT NULL
   OR s.reblogged IS NOT NULL
   OR s.muted IS NOT NULL
   OR s.bookmarked IS NOT NULL
   OR s.pinned IS NOT NULL;

-- These compatibility columns remain so existing custom SELECT layouts keep
-- their shape, but they are no longer a source of truth.
UPDATE statuses
SET favourited = NULL,
    reblogged = NULL,
    muted = NULL,
    bookmarked = NULL;

-- Ensure every normalized mapping has a valid parent before adding the
-- composite FK. This also repairs databases whose legacy tags table was only
-- partially populated from tags_json.
INSERT OR IGNORE INTO tags (name, server_domain)
SELECT DISTINCT json_extract(tag.value, '$.name'), s.server_domain
FROM statuses s, json_each(s.tags_json) tag
WHERE s.tags_json IS NOT NULL
  AND json_valid(s.tags_json)
  AND json_extract(tag.value, '$.name') IS NOT NULL;

CREATE TABLE status_tags (
    status_id TEXT NOT NULL,
    server_domain TEXT NOT NULL,
    tag_name TEXT NOT NULL,
    PRIMARY KEY (status_id, server_domain, tag_name),
    FOREIGN KEY (status_id, server_domain)
        REFERENCES statuses(id, server_domain) ON DELETE CASCADE,
    FOREIGN KEY (tag_name, server_domain)
        REFERENCES tags(name, server_domain) ON DELETE CASCADE
);

CREATE INDEX idx_status_tags_tag
    ON status_tags(tag_name, server_domain);

INSERT OR IGNORE INTO status_tags (status_id, server_domain, tag_name)
SELECT s.id, s.server_domain, json_extract(tag.value, '$.name')
FROM statuses s, json_each(s.tags_json) tag
WHERE s.tags_json IS NOT NULL
  AND json_valid(s.tags_json)
  AND json_extract(tag.value, '$.name') IS NOT NULL;

-- Repair legacy orphans while rebuilding the account-scoped notification key
-- and explicit cascade policy.
ALTER TABLE notifications RENAME TO notifications_legacy_021;

CREATE TABLE notifications (
    id TEXT NOT NULL,
    server_domain TEXT NOT NULL,
    account_acct TEXT NOT NULL,
    notification_type TEXT NOT NULL,
    created_at TEXT NOT NULL,
    account_id TEXT NOT NULL,
    status_id TEXT,
    read_at TEXT,
    fetched_at TEXT NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (id, server_domain, account_acct),
    FOREIGN KEY (account_acct)
        REFERENCES login_accounts(acct) ON DELETE CASCADE,
    FOREIGN KEY (account_id, server_domain)
        REFERENCES accounts(id, server_domain) ON DELETE CASCADE,
    FOREIGN KEY (status_id, server_domain)
        REFERENCES statuses(id, server_domain) ON DELETE CASCADE
);

INSERT INTO notifications (
    id, server_domain, account_acct, notification_type, created_at,
    account_id, status_id, read_at, fetched_at
)
SELECT
    n.id,
    n.server_domain,
    COALESCE(
        n.account_acct,
        (
            SELECT la.acct
            FROM login_accounts la
            WHERE la.server_domain = n.server_domain
            ORDER BY la.is_active DESC, la.acct
            LIMIT 1
        )
    ),
    n.notification_type,
    n.created_at,
    n.account_id,
    n.status_id,
    n.read_at,
    n.fetched_at
FROM notifications_legacy_021 n
JOIN accounts actor
  ON actor.id = n.account_id AND actor.server_domain = n.server_domain
LEFT JOIN statuses status
  ON status.id = n.status_id AND status.server_domain = n.server_domain
WHERE (n.status_id IS NULL OR status.id IS NOT NULL)
  AND COALESCE(
        n.account_acct,
        (
            SELECT la.acct
            FROM login_accounts la
            WHERE la.server_domain = n.server_domain
            ORDER BY la.is_active DESC, la.acct
            LIMIT 1
        )
      ) IS NOT NULL;

DROP TABLE notifications_legacy_021;

CREATE INDEX idx_notifications_created
    ON notifications(account_acct, created_at DESC);
CREATE INDEX idx_notifications_unread
    ON notifications(account_acct, read_at) WHERE read_at IS NULL;

ALTER TABLE timeline_entries RENAME TO timeline_entries_legacy_021;

CREATE TABLE timeline_entries (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    timeline_type TEXT NOT NULL,
    server_domain TEXT NOT NULL,
    status_id TEXT NOT NULL,
    account_acct TEXT NOT NULL,
    position_at TEXT NOT NULL,
    UNIQUE(timeline_type, server_domain, status_id, account_acct),
    FOREIGN KEY (status_id, server_domain)
        REFERENCES statuses(id, server_domain) ON DELETE CASCADE,
    FOREIGN KEY (account_acct)
        REFERENCES login_accounts(acct) ON DELETE CASCADE
);

INSERT INTO timeline_entries (
    id, timeline_type, server_domain, status_id, account_acct, position_at
)
SELECT te.id, te.timeline_type, te.server_domain, te.status_id, te.account_acct, te.position_at
FROM timeline_entries_legacy_021 te
JOIN statuses s ON s.id = te.status_id AND s.server_domain = te.server_domain
JOIN login_accounts la ON la.acct = te.account_acct;

DROP TABLE timeline_entries_legacy_021;

CREATE INDEX idx_timeline_entries_lookup
    ON timeline_entries(timeline_type, account_acct, position_at DESC);
CREATE INDEX idx_timeline_entries_account_latest
    ON timeline_entries(account_acct, position_at DESC, id DESC);
CREATE INDEX idx_timeline_entries_status_latest
    ON timeline_entries(server_domain, status_id, position_at DESC);
