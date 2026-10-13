-- Resumable uploads (tus). Each row tracks one file in flight; its bytes live
-- in <data>/tmp/uploads/<id> until it's complete and moved into place.

CREATE TABLE uploads (
    id          TEXT    NOT NULL PRIMARY KEY,
    user_id     TEXT    NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    server_id   TEXT    NOT NULL REFERENCES servers (id) ON DELETE CASCADE,
    -- Destination folder, relative to the server's root.
    dest_path   TEXT    NOT NULL,
    filename    TEXT    NOT NULL,
    total_size  INTEGER NOT NULL CHECK (total_size >= 0),
    -- Bytes received and synced to disk.
    received    INTEGER NOT NULL DEFAULT 0 CHECK (received >= 0 AND received <= total_size),
    -- Expected SHA-256 of the whole file (hex), when the client sent one.
    sha256      TEXT,
    -- When a file with the same name is already there.
    on_conflict TEXT    NOT NULL CHECK (on_conflict IN ('rename', 'overwrite', 'skip')),
    -- Extract a .zip into the destination once it's in place.
    extract     INTEGER NOT NULL DEFAULT 0 CHECK (extract IN (0, 1)),
    status      TEXT    NOT NULL CHECK (status IN ('uploading', 'finishing', 'done', 'failed', 'cancelled', 'expired')),
    -- Outcome, for display (final path, or why it failed).
    message     TEXT,
    created_at  INTEGER NOT NULL,
    updated_at  INTEGER NOT NULL,
    -- Unfinished uploads are discarded after this (slides with each chunk).
    expires_at  INTEGER NOT NULL
) STRICT;

CREATE INDEX uploads_user_status ON uploads (user_id, status);
CREATE INDEX uploads_status_expires ON uploads (status, expires_at);
