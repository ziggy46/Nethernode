-- A hostname can have a route on each listener (and one for all of them),
-- so every server with a port of its own can have its own `*` route.
-- Before, hostnames were unique across all listeners.
--
-- (0016 is on the edge branch; numbers only go up, and either order of
-- applying them works.)
CREATE TABLE routes_new (
    id          TEXT    NOT NULL PRIMARY KEY,
    server_id   TEXT    REFERENCES servers (id) ON DELETE CASCADE,
    hostname    TEXT    NOT NULL,
    listener_id TEXT    REFERENCES proxy_listeners (id) ON DELETE CASCADE,
    priority    INTEGER NOT NULL DEFAULT 0,
    CHECK (server_id IS NOT NULL OR hostname LIKE '*.%')
) STRICT;

INSERT INTO routes_new (id, server_id, hostname, listener_id, priority)
SELECT id, server_id, hostname, listener_id, priority FROM routes;

DROP TABLE routes;
ALTER TABLE routes_new RENAME TO routes;

CREATE INDEX routes_server_id ON routes (server_id);
-- NULL listener_id (every listener) counts as one value.
CREATE UNIQUE INDEX routes_hostname_listener ON routes (hostname, IFNULL(listener_id, ''));

-- The server a listener was made for ("its own port"), so deleting the
-- server can remove the listener too. NULL for listeners made by hand.
ALTER TABLE proxy_listeners ADD COLUMN made_for_server TEXT;
