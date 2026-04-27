-- Track which fediverse server software each cached server is running.
ALTER TABLE servers ADD COLUMN server_kind TEXT NOT NULL DEFAULT 'mastodon';
