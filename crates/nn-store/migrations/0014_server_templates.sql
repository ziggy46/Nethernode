-- Server templates: a server's files without its world (an archive kept
-- beside the panel's backups, `template-<id>.tar.zst`) and its settings, to
-- start new servers from.
CREATE TABLE server_templates (
    id            TEXT    PRIMARY KEY,
    name          TEXT    NOT NULL,
    -- The server it was made from, for display (it may be gone since).
    source_name   TEXT    NOT NULL,
    loader        TEXT    NOT NULL,
    mc_version    TEXT    NOT NULL,
    memory_mb     INTEGER NOT NULL,
    -- The server's settings, as a server row in JSON.
    server_json   TEXT    NOT NULL,
    archive_bytes INTEGER NOT NULL,
    created_at    INTEGER NOT NULL
);
