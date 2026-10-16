-- Which machine holds each backup's archive (a backup outlives its server).
ALTER TABLE backups ADD COLUMN node_id TEXT NOT NULL DEFAULT 'local';
