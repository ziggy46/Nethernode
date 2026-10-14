-- Account management: one-time passwords that must be changed, and TOTP
-- two-factor sign-in with recovery codes.

ALTER TABLE users ADD COLUMN must_change_password INTEGER NOT NULL DEFAULT 0;
-- users.totp_secret holds the active secret, sealed with the data key.
-- A secret shown for enrollment but not yet confirmed with a code, sealed.
ALTER TABLE users ADD COLUMN totp_pending TEXT;
ALTER TABLE users ADD COLUMN totp_enabled_at INTEGER;
-- The last TOTP time step accepted, so a code can't be used twice.
ALTER TABLE users ADD COLUMN totp_last_step INTEGER;

CREATE TABLE recovery_codes (
    id        TEXT    NOT NULL PRIMARY KEY,
    user_id   TEXT    NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    -- SHA-256 of the normalized code, hex encoded.
    code_hash TEXT    NOT NULL,
    used_at   INTEGER
) STRICT;

CREATE INDEX recovery_codes_user_id ON recovery_codes (user_id);
