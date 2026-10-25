-- Bedrock servers are no longer fronted by a UDP listener: players connect to
-- their own ports (see 0020). Drop the listeners the first edge builds made;
-- their routes go with them.
DELETE FROM proxy_listeners WHERE protocol = 'bedrock';
