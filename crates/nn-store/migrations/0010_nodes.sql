-- Machines that run servers: this one, and agents that joined it.

CREATE TABLE nodes (
    id           TEXT    NOT NULL PRIMARY KEY,
    name         TEXT    NOT NULL,
    -- The agent's Noise public key, hex encoded. NULL for this machine, and
    -- for a node whose agent hasn't joined yet.
    public_key   TEXT    UNIQUE,
    -- The agent's NetherNode version, as it last said.
    version      TEXT,
    -- What the agent last reported about its machine (JSON).
    host_json    TEXT,
    last_seen_at INTEGER,
    created_at   INTEGER NOT NULL
) STRICT;

CREATE UNIQUE INDEX nodes_name ON nodes (name COLLATE NOCASE);

INSERT INTO nodes (id, name, created_at)
VALUES ('local', 'This machine', CAST(strftime('%s', 'now') AS INTEGER) * 1000);

-- One-time codes for an agent's first connection. Only the secret's
-- SHA-256 is kept.
CREATE TABLE node_join_codes (
    id          TEXT    NOT NULL PRIMARY KEY,
    node_id     TEXT    NOT NULL REFERENCES nodes (id) ON DELETE CASCADE,
    secret_hash TEXT    NOT NULL,
    expires_at  INTEGER NOT NULL
) STRICT;

CREATE INDEX node_join_codes_node_id ON node_join_codes (node_id);
