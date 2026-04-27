-- Track which fediverse server software each login_account is running.
-- 'mastodon' (default), 'paon', 'misskey'
ALTER TABLE login_accounts ADD COLUMN server_kind TEXT NOT NULL DEFAULT 'mastodon';
