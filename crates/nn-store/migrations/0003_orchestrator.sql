-- Fields the orchestrator needs to create and run containers.

-- Host port for RCON, bound to 127.0.0.1 (allocated from the same range as internal_port).
ALTER TABLE servers ADD COLUMN rcon_port INTEGER CHECK (rcon_port BETWEEN 1 AND 65535);
CREATE UNIQUE INDEX servers_rcon_port ON servers (rcon_port);
-- Docker container ID, once created.
ALTER TABLE servers ADD COLUMN container_id TEXT;
-- When the admin accepted the Minecraft EULA in the create wizard (unix ms).
ALTER TABLE servers ADD COLUMN eula_accepted_at INTEGER;
-- Card artwork only (nether, overworld, end, sky); never affects the world.
ALTER TABLE servers ADD COLUMN card_theme TEXT NOT NULL DEFAULT 'nether'
    CHECK (card_theme IN ('nether', 'overworld', 'end', 'sky'));
ALTER TABLE servers ADD COLUMN restart_on_crash INTEGER NOT NULL DEFAULT 1;
ALTER TABLE servers ADD COLUMN use_aikar_flags INTEGER NOT NULL DEFAULT 1;
ALTER TABLE servers ADD COLUMN motd TEXT;
ALTER TABLE servers ADD COLUMN max_players INTEGER NOT NULL DEFAULT 20 CHECK (max_players BETWEEN 1 AND 10000);
ALTER TABLE servers ADD COLUMN last_started_at INTEGER;
