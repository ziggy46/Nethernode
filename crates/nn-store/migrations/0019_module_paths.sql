-- A module's web page on the shared address for modules' pages (a setting,
-- like map.example.com), at /<web_path>/. NULL: on its own address
-- (web_host) instead.
ALTER TABLE server_modules ADD COLUMN web_path TEXT;
