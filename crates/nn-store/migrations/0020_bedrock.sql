-- Bedrock servers are reached over UDP (RakNet), so their public ports are
-- listeners of a second kind. Java listeners stay as they were.
ALTER TABLE proxy_listeners ADD COLUMN protocol TEXT NOT NULL DEFAULT 'java'
    CHECK (protocol IN ('java', 'bedrock'));
