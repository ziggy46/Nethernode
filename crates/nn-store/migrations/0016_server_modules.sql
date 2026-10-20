-- Modules: helper containers that run next to a server (a web map, for
-- example), each made from a built-in template. Their files live in the
-- server's folder under modules/<name>/.
CREATE TABLE server_modules (
    id            TEXT    NOT NULL PRIMARY KEY,
    server_id     TEXT    NOT NULL REFERENCES servers (id) ON DELETE CASCADE,
    -- The template it's made from (e.g. bluemap).
    template      TEXT    NOT NULL,
    -- Its folder and container name; unique on its server.
    name          TEXT    NOT NULL,
    -- The template's settings, as filled in.
    settings_json TEXT    NOT NULL DEFAULT '{}',
    -- When it runs: alongside the server, all the time, or only when started
    -- by hand.
    lifecycle     TEXT    NOT NULL CHECK (lifecycle IN ('with-server', 'always', 'manual')),
    memory_mb     INTEGER NOT NULL,
    -- Host port its web page is published on, if it has one.
    web_port      INTEGER,
    created_at    INTEGER NOT NULL,
    updated_at    INTEGER NOT NULL,
    UNIQUE (server_id, name)
) STRICT;

CREATE INDEX server_modules_server ON server_modules (server_id);
