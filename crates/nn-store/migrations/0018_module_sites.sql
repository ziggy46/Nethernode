-- A module's web page on an address of its own, for anyone to open:
-- `web_host` is a prefix put in front of the server's address (`map` makes
-- map.<server address>) or, with a dot in it, a whole hostname.
ALTER TABLE server_modules ADD COLUMN web_host TEXT;
ALTER TABLE server_modules ADD COLUMN web_public INTEGER NOT NULL DEFAULT 0 CHECK (web_public IN (0, 1));
