-- Servers, Smart Proxy listeners and routes, and player tracking.
-- IDs are ULIDs stored as TEXT; timestamps are INTEGER unix milliseconds.

CREATE TABLE servers (
    id                     TEXT    NOT NULL PRIMARY KEY,
    name                   TEXT    NOT NULL,
    slug                   TEXT    NOT NULL UNIQUE,
    status                 TEXT    NOT NULL DEFAULT 'stopped'
        CHECK (status IN ('installing', 'stopped', 'starting', 'running', 'stopping', 'sleeping', 'crashed')),
    mc_version             TEXT    NOT NULL,
    loader                 TEXT    NOT NULL,
    loader_version         TEXT,
    image                  TEXT    NOT NULL,
    java_version           INTEGER,
    memory_mb              INTEGER NOT NULL,
    cpu_limit              REAL,
    -- Port the container's Minecraft server listens on, bound to 127.0.0.1.
    internal_port          INTEGER NOT NULL UNIQUE CHECK (internal_port BETWEEN 1 AND 65535),
    public_port            INTEGER CHECK (public_port BETWEEN 1 AND 65535),
    hostname               TEXT UNIQUE,
    auto_start             INTEGER NOT NULL DEFAULT 0,
    auto_stop_idle_minutes INTEGER,
    sleep_when_empty       INTEGER NOT NULL DEFAULT 0,
    rcon_password_enc      TEXT,
    env_json               TEXT    NOT NULL DEFAULT '{}',
    created_at             INTEGER NOT NULL,
    updated_at             INTEGER NOT NULL
) STRICT;

CREATE TABLE proxy_listeners (
    id             TEXT    NOT NULL PRIMARY KEY,
    bind_addr      TEXT    NOT NULL,
    port           INTEGER NOT NULL CHECK (port BETWEEN 1 AND 65535),
    enabled        INTEGER NOT NULL DEFAULT 1,
    proxy_protocol INTEGER NOT NULL DEFAULT 0,
    UNIQUE (bind_addr, port)
) STRICT;

-- Hostname patterns:
--   exact     `play.example.com`  → server_id
--   wildcard  `*.example.com`     → server_id, or the server whose slug is the
--                                   subdomain when server_id is NULL
--   default   `*`                 → server_id, for anything unmatched
-- listener_id NULL means the route applies on every listener. Among routes
-- of the same kind, higher priority wins, then listener-specific routes.
CREATE TABLE routes (
    id          TEXT    NOT NULL PRIMARY KEY,
    server_id   TEXT    REFERENCES servers (id) ON DELETE CASCADE,
    hostname    TEXT    NOT NULL UNIQUE,
    listener_id TEXT    REFERENCES proxy_listeners (id) ON DELETE CASCADE,
    priority    INTEGER NOT NULL DEFAULT 0,
    CHECK (server_id IS NOT NULL OR hostname LIKE '*.%')
) STRICT;

CREATE INDEX routes_server_id ON routes (server_id);

CREATE TABLE players (
    uuid          TEXT    NOT NULL PRIMARY KEY,
    last_username TEXT    NOT NULL,
    first_seen    INTEGER NOT NULL,
    last_seen     INTEGER NOT NULL,
    notes         TEXT
) STRICT;

-- `uuid` is NULL when the client's protocol doesn't send one (before 1.19.1).
CREATE TABLE player_sessions (
    id        TEXT    NOT NULL PRIMARY KEY,
    server_id TEXT    NOT NULL REFERENCES servers (id) ON DELETE CASCADE,
    uuid      TEXT,
    username  TEXT    NOT NULL,
    ip        TEXT,
    joined_at INTEGER NOT NULL,
    left_at   INTEGER
) STRICT;

CREATE INDEX player_sessions_server_open ON player_sessions (server_id, left_at);
CREATE INDEX player_sessions_uuid ON player_sessions (uuid);
