-- Relay sessions, opened by the device flow sign-in. Only the SHA-256 of each
-- token is stored, so the table cannot be used to sign in as anyone.
CREATE TABLE sessions (
    token_hash BYTEA PRIMARY KEY,
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    expires_at TIMESTAMPTZ NOT NULL,
    last_seen_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- GitHub logins have up to 39 characters.
ALTER TABLE users ALTER COLUMN username TYPE VARCHAR(64);

-- Tokens were stored in clear here and never expired. Every one of them is
-- void now: users sign in again and get a session.
DROP INDEX IF EXISTS idx_users_auth_token;
ALTER TABLE users DROP COLUMN auth_token;
