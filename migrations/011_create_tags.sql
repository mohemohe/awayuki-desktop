CREATE TABLE IF NOT EXISTS tags (
    name TEXT NOT NULL,
    server_domain TEXT NOT NULL,
    PRIMARY KEY (name, server_domain)
);

CREATE INDEX IF NOT EXISTS idx_tags_name ON tags(name);

INSERT OR IGNORE INTO tags (name, server_domain)
SELECT DISTINCT json_extract(j.value, '$.name'), s.server_domain
FROM statuses s, json_each(s.tags_json) j
WHERE s.tags_json IS NOT NULL;
