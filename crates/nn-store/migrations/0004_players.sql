-- Player management: the server management protocol, timed and global bans,
-- and cached Mojang profile lookups.

-- Host port (127.0.0.1) for the Minecraft server management protocol, and its
-- secret sealed with the data key. Allocated when the container is next created.
ALTER TABLE servers ADD COLUMN mgmt_port INTEGER CHECK (mgmt_port BETWEEN 1 AND 65535);
CREATE UNIQUE INDEX servers_mgmt_port ON servers (mgmt_port);
ALTER TABLE servers ADD COLUMN mgmt_secret_enc TEXT;

-- Bans NetherNode placed. Minecraft keeps its own lists; these rows add what
-- it can't do everywhere: expiry (lifted by NetherNode's scheduler) and bans
-- that apply to every server (`server_id` NULL), including ones created later.
CREATE TABLE player_bans (
    id         TEXT    NOT NULL PRIMARY KEY,
    server_id  TEXT    REFERENCES servers (id) ON DELETE CASCADE,
    uuid       TEXT,
    name       TEXT    NOT NULL,
    -- Also banned by IP address.
    ip         TEXT,
    reason     TEXT,
    -- NULL means permanent.
    expires_at INTEGER,
    created_by TEXT    REFERENCES users (id) ON DELETE SET NULL,
    created_at INTEGER NOT NULL,
    -- When the ban was lifted (expired or pardoned); NULL while active.
    lifted_at  INTEGER
) STRICT;

CREATE INDEX player_bans_active ON player_bans (lifted_at, expires_at);
CREATE INDEX player_bans_uuid ON player_bans (uuid);

-- Username lookups against Mojang. `uuid` NULL records that no account has
-- that name, so repeated lookups don't hit the rate limit.
CREATE TABLE profile_cache (
    name_key   TEXT    NOT NULL PRIMARY KEY, -- lowercase username
    uuid       TEXT,
    name       TEXT,                         -- correct capitalization
    fetched_at INTEGER NOT NULL
) STRICT;
