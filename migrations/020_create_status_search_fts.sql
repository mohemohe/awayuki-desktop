-- Keep a trigram FTS index beside the status cache so local search can use an
-- indexed candidate set while retaining the existing substring semantics.
--
-- statuses has a composite primary key, so SQLite may rewrite its implicit
-- rowid during VACUUM. A separate stable docid mapping keeps FTS references
-- valid across the user-facing vacuum operation.
CREATE INDEX IF NOT EXISTS idx_statuses_domain_created
    ON statuses(server_domain, created_at DESC, id DESC);

CREATE INDEX IF NOT EXISTS idx_statuses_visibility_created
    ON statuses(visibility, created_at DESC, server_domain DESC, id DESC);

CREATE TABLE IF NOT EXISTS status_search_documents (
    docid INTEGER PRIMARY KEY,
    status_id TEXT NOT NULL,
    server_domain TEXT NOT NULL,
    UNIQUE(status_id, server_domain)
);

CREATE VIRTUAL TABLE IF NOT EXISTS status_search_fts USING fts5(
    content,
    spoiler_text,
    uri,
    url,
    tags,
    account_acct,
    account_display_name,
    tokenize = 'trigram'
);

DELETE FROM status_search_fts;
DELETE FROM status_search_documents;

INSERT INTO status_search_documents (status_id, server_domain)
SELECT id, server_domain
FROM statuses
ORDER BY created_at, server_domain, id;

INSERT INTO status_search_fts (
    rowid,
    content,
    spoiler_text,
    uri,
    url,
    tags,
    account_acct,
    account_display_name
)
SELECT
    d.docid,
    s.content,
    s.spoiler_text,
    s.uri,
    COALESCE(s.url, ''),
    COALESCE(s.tags_json, ''),
    COALESCE(a.acct, ''),
    COALESCE(a.display_name, '')
FROM status_search_documents d
JOIN statuses s
  ON s.id = d.status_id
 AND s.server_domain = d.server_domain
LEFT JOIN accounts a
  ON a.id = s.account_id
 AND a.server_domain = s.server_domain;

CREATE TRIGGER IF NOT EXISTS status_search_fts_status_insert
AFTER INSERT ON statuses
BEGIN
    INSERT INTO status_search_documents (status_id, server_domain)
    VALUES (NEW.id, NEW.server_domain)
    ON CONFLICT(status_id, server_domain) DO NOTHING;

    INSERT INTO status_search_fts (
        rowid,
        content,
        spoiler_text,
        uri,
        url,
        tags,
        account_acct,
        account_display_name
    )
    SELECT
        d.docid,
        NEW.content,
        NEW.spoiler_text,
        NEW.uri,
        COALESCE(NEW.url, ''),
        COALESCE(NEW.tags_json, ''),
        COALESCE(a.acct, ''),
        COALESCE(a.display_name, '')
    FROM status_search_documents d
    LEFT JOIN accounts a
      ON a.id = NEW.account_id
     AND a.server_domain = NEW.server_domain
    WHERE d.status_id = NEW.id
      AND d.server_domain = NEW.server_domain;
END;

CREATE TRIGGER IF NOT EXISTS status_search_fts_status_update
AFTER UPDATE OF id, server_domain, account_id, content, spoiler_text, uri, url, tags_json ON statuses
BEGIN
    DELETE FROM status_search_fts
     WHERE rowid = (
         SELECT docid
           FROM status_search_documents
          WHERE status_id = OLD.id
            AND server_domain = OLD.server_domain
     );

    UPDATE status_search_documents
       SET status_id = NEW.id,
           server_domain = NEW.server_domain
     WHERE status_id = OLD.id
       AND server_domain = OLD.server_domain;

    INSERT INTO status_search_fts (
        rowid,
        content,
        spoiler_text,
        uri,
        url,
        tags,
        account_acct,
        account_display_name
    )
    SELECT
        d.docid,
        NEW.content,
        NEW.spoiler_text,
        NEW.uri,
        COALESCE(NEW.url, ''),
        COALESCE(NEW.tags_json, ''),
        COALESCE(a.acct, ''),
        COALESCE(a.display_name, '')
    FROM status_search_documents d
    LEFT JOIN accounts a
      ON a.id = NEW.account_id
     AND a.server_domain = NEW.server_domain
    WHERE d.status_id = NEW.id
      AND d.server_domain = NEW.server_domain;
END;

CREATE TRIGGER IF NOT EXISTS status_search_fts_status_delete
AFTER DELETE ON statuses
BEGIN
    DELETE FROM status_search_fts
     WHERE rowid = (
         SELECT docid
           FROM status_search_documents
          WHERE status_id = OLD.id
            AND server_domain = OLD.server_domain
     );

    DELETE FROM status_search_documents
     WHERE status_id = OLD.id
       AND server_domain = OLD.server_domain;
END;

CREATE TRIGGER IF NOT EXISTS status_search_fts_account_update
AFTER UPDATE OF acct, display_name ON accounts
BEGIN
    UPDATE status_search_fts
       SET account_acct = NEW.acct,
           account_display_name = NEW.display_name
     WHERE rowid IN (
         SELECT d.docid
           FROM status_search_documents d
           JOIN statuses s
             ON s.id = d.status_id
            AND s.server_domain = d.server_domain
          WHERE s.account_id = NEW.id
            AND s.server_domain = NEW.server_domain
     );
END;

CREATE TRIGGER IF NOT EXISTS status_search_fts_account_insert
AFTER INSERT ON accounts
BEGIN
    UPDATE status_search_fts
       SET account_acct = NEW.acct,
           account_display_name = NEW.display_name
     WHERE rowid IN (
         SELECT d.docid
           FROM status_search_documents d
           JOIN statuses s
             ON s.id = d.status_id
            AND s.server_domain = d.server_domain
          WHERE s.account_id = NEW.id
            AND s.server_domain = NEW.server_domain
     );
END;

CREATE TRIGGER IF NOT EXISTS status_search_fts_account_delete
AFTER DELETE ON accounts
BEGIN
    UPDATE status_search_fts
       SET account_acct = '',
           account_display_name = ''
     WHERE rowid IN (
         SELECT d.docid
           FROM status_search_documents d
           JOIN statuses s
             ON s.id = d.status_id
            AND s.server_domain = d.server_domain
          WHERE s.account_id = OLD.id
            AND s.server_domain = OLD.server_domain
     );
END;
