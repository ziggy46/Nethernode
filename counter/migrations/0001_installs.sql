-- One row per install that reported in: its random ID, its version, and the
-- day (days since 1970-01-01, UTC) of its last report. Nothing else.
CREATE TABLE installs (
    id       TEXT    NOT NULL PRIMARY KEY,
    version  TEXT    NOT NULL,
    last_day INTEGER NOT NULL
);

CREATE INDEX installs_last_day ON installs (last_day);
