-- Which machine each server runs on. Servers on other machines are mirrored
-- here from their agent, without their secrets (those stay with the agent).
-- Not a foreign key: SQLite can't add one to an existing table with a
-- default. Nodes with servers can't be removed (checked in code).
ALTER TABLE servers ADD COLUMN node_id TEXT NOT NULL DEFAULT 'local';

CREATE INDEX servers_node_id ON servers (node_id);
