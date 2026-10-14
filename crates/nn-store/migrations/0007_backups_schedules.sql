-- Backups of server folders, and scheduled actions (restarts, backups,
-- commands, broadcasts).

-- Archives live in <data>/backups/<file_name>. A backup outlives its server
-- (server_id becomes NULL) so a deleted server's worlds aren't lost with it.
CREATE TABLE backups (
    id          TEXT    NOT NULL PRIMARY KEY,
    server_id   TEXT    REFERENCES servers (id) ON DELETE SET NULL,
    -- The server's name when the backup was made.
    server_name TEXT    NOT NULL,
    file_name   TEXT    NOT NULL UNIQUE,
    size_bytes  INTEGER NOT NULL DEFAULT 0,
    -- Files inside, and their size before compression.
    files       INTEGER NOT NULL DEFAULT 0,
    raw_bytes   INTEGER NOT NULL DEFAULT 0,
    kind        TEXT    NOT NULL CHECK (kind IN ('manual', 'scheduled', 'pre-restore')),
    status      TEXT    NOT NULL CHECK (status IN ('creating', 'ready', 'failed')),
    -- Why it failed, or a note.
    message     TEXT,
    created_by  TEXT    REFERENCES users (id) ON DELETE SET NULL,
    created_at  INTEGER NOT NULL,
    finished_at INTEGER
) STRICT;

CREATE INDEX backups_server ON backups (server_id, created_at);

CREATE TABLE schedules (
    id           TEXT    NOT NULL PRIMARY KEY,
    server_id    TEXT    NOT NULL REFERENCES servers (id) ON DELETE CASCADE,
    name         TEXT    NOT NULL,
    -- Cron expression (5 or 6 fields), in `timezone`.
    cron         TEXT    NOT NULL,
    -- IANA zone, e.g. America/New_York.
    timezone     TEXT    NOT NULL,
    action       TEXT    NOT NULL CHECK (action IN ('restart', 'backup', 'command', 'broadcast', 'start', 'stop')),
    -- Action details as JSON: the command or message, warning times,
    -- backups to keep.
    payload_json TEXT    NOT NULL DEFAULT '{}',
    enabled      INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0, 1)),
    next_run_at  INTEGER,
    last_run_at  INTEGER,
    -- What happened last time, for display.
    last_result  TEXT,
    created_at   INTEGER NOT NULL,
    updated_at   INTEGER NOT NULL
) STRICT;

CREATE INDEX schedules_server ON schedules (server_id);
