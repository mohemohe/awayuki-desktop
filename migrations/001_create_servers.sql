CREATE TABLE IF NOT EXISTS servers (
    domain TEXT PRIMARY KEY,
    streaming_url TEXT NOT NULL,
    version TEXT,
    max_characters INTEGER DEFAULT 500,
    instance_json TEXT
);
