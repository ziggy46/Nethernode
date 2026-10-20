-- The modpack version a backup's files belong to, so restoring it puts the
-- server back on that version (and Minecraft version). NULL for servers
-- that aren't modpacks.
ALTER TABLE backups ADD COLUMN modpack_json TEXT;
ALTER TABLE backups ADD COLUMN mc_version TEXT;
