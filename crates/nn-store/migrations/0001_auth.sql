-- Users, sessions, settings, and the audit log.
-- IDs are ULIDs stored as TEXT; timestamps are INTEGER unix milliseconds.

CREATE TABLE users (
    id            TEXT    NOT NULL PRIMARY KEY,
    username      TEXT    NOT NULL UNIQUE COLLATE NOCASE,
    password_hash TEXT    NOT NULL,
    role          TEXT    NOT NULL CHECK (role IN ('owner', 'admin', 'moderator', 'viewer')),
    totp_secret   TEXT,
    created_at    INTEGER NOT NULL,
    last_login_at INTEGER
) STRICT;

CREATE TABLE sessions (
    id         TEXT    NOT NULL PRIMARY KEY,
    user_id    TEXT    NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    -- SHA-256 of the session token, hex encoded. The token itself is never stored.
    token_hash TEXT    NOT NULL UNIQUE,
    user_agent TEXT,
    ip         TEXT,
    created_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL
) STRICT;

CREATE INDEX sessions_user_id ON sessions (user_id);
CREATE INDEX sessions_expires_at ON sessions (expires_at);

CREATE TABLE settings (
    key        TEXT NOT NULL PRIMARY KEY,
    value_json TEXT NOT NULL
) STRICT;

CREATE TABLE audit_log (
    id           TEXT    NOT NULL PRIMARY KEY,
    user_id      TEXT    REFERENCES users (id) ON DELETE SET NULL,
    action       TEXT    NOT NULL,
    target_type  TEXT,
    target_id    TEXT,
    details_json TEXT,
    created_at   INTEGER NOT NULL
) STRICT;

CREATE INDEX audit_log_created_at ON audit_log (created_at);
