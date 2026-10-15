-- Per-account preferences.

-- The panel's color theme: follow the device, or always dark or light.
ALTER TABLE users ADD COLUMN theme TEXT NOT NULL DEFAULT 'system'
    CHECK (theme IN ('system', 'dark', 'light'));
