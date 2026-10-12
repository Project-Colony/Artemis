-- Relay sessions, opened by the device flow sign-in. Only the SHA-256 of each
-- token is stored, so the table cannot be used to sign in as anyone.
CREATE TABLE sessions (
    token_hash BYTEA PRIMARY KEY,
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    expires_at TIMESTAMPTZ NOT NULL,
    last_seen_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- GitHub logins have up to 39 characters, and GitHub ignores their case, so
-- usernames are unique whatever their case. Accounts whose names differ only
-- in case give them up for a name with a `#`, which no GitHub login can have,
-- until their users sign in again.
ALTER TABLE users ALTER COLUMN username TYPE VARCHAR(64);
UPDATE users SET username = '#' || github_id
WHERE lower(username) IN (SELECT lower(username) FROM users GROUP BY 1 HAVING COUNT(*) > 1);
ALTER TABLE users DROP CONSTRAINT users_username_key;
CREATE UNIQUE INDEX users_username_lower_key ON users (lower(username));

-- Tokens were stored in clear here and never expired. Every one of them is
-- void now: users sign in again and get a session.
DROP INDEX IF EXISTS idx_users_auth_token;
ALTER TABLE users DROP COLUMN auth_token;
