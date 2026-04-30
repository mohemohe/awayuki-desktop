-- Persist Bluesky app password so we can re-create the session when the
-- stored access/refresh JWTs are rejected. NULL for non-Bluesky accounts and
-- for Bluesky rows created before this migration (those will need re-login).
ALTER TABLE login_accounts ADD COLUMN app_password TEXT;
