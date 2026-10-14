-- Servers installed from a modpack (Modrinth or CurseForge): which pack and
-- version, as JSON. NULL for servers that aren't modpacks.
ALTER TABLE servers ADD COLUMN modpack_json TEXT;
