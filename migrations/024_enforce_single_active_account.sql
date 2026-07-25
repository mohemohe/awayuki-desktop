-- Keep the active-account invariant in SQLite instead of relying on every
-- caller to perform the two UPDATE statements correctly. Historical debug
-- databases may contain more than one active row, so retain the
-- lexicographically first account deterministically before adding the guard.
UPDATE login_accounts
   SET is_active = 0
 WHERE is_active = 1
   AND acct <> (
       SELECT acct
         FROM login_accounts
        WHERE is_active = 1
        ORDER BY acct
        LIMIT 1
   );

CREATE UNIQUE INDEX idx_login_accounts_single_active
    ON login_accounts(is_active)
 WHERE is_active = 1;
