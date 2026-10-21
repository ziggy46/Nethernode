-- Applied by edge builds 0.2.11-edge.44 and later, then the shared address
-- for modules' pages was taken out again: nothing uses this column. The
-- migration stays so its number is never reused.
-- A module's web page on the shared address for modules' pages (a setting,
-- like map.example.com), at /<web_path>/. NULL: on its own address
-- (web_host) instead.
ALTER TABLE server_modules ADD COLUMN web_path TEXT;
