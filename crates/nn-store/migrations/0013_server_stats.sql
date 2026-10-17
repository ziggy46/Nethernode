-- Servers' resource use over time, summarised per minute (kept a few days)
-- and per hour (kept about a month). Sums rather than averages, so a period
-- can be added to as samples arrive and periods can be combined.
CREATE TABLE server_stats (
    server_id      TEXT    NOT NULL REFERENCES servers (id) ON DELETE CASCADE,
    -- Length in seconds: 60 or 3600.
    period         INTEGER NOT NULL,
    -- Start, unix ms.
    at_ms          INTEGER NOT NULL,
    -- Samples taken (about one a second while the server runs).
    samples        INTEGER NOT NULL,
    cpu_sum        REAL    NOT NULL,
    cpu_max        REAL    NOT NULL,
    memory_sum     INTEGER NOT NULL,
    memory_max     INTEGER NOT NULL,
    memory_limit   INTEGER NOT NULL,
    -- Bytes moved during the period.
    net_rx         INTEGER NOT NULL,
    net_tx         INTEGER NOT NULL,
    disk_read      INTEGER NOT NULL,
    disk_write     INTEGER NOT NULL,
    players_max    INTEGER,
    -- Samples with tick timings, and their sums and extremes.
    tick_samples   INTEGER NOT NULL,
    mspt_sum       REAL    NOT NULL,
    mspt_max       REAL,
    tps_sum        REAL    NOT NULL,
    tps_min        REAL,
    -- Samples taken while Minecraft had paused the empty server.
    paused_samples INTEGER NOT NULL,
    PRIMARY KEY (server_id, period, at_ms)
) WITHOUT ROWID;
