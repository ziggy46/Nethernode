---
title: Discord notifications
description: Post server events to a Discord channel.
---

NetherNode can post to a Discord channel when servers crash, start or stop
(including restarts), when players join, and when a backup fails. Each can
be turned on or off.

1. In Discord, open the channel's settings, then **Integrations › Webhooks
   › New Webhook**, and copy its URL.
2. In NetherNode, open **Settings › Discord notifications**, paste it, and
   save. Use **Send a test** to check.

Events a few seconds apart are sent together, so a restart is one message
and a group of friends joining is one line. If Discord asks NetherNode to
slow down, it waits. If a message can't be delivered, the reason shows in
Settings.

The webhook URL works like a password for the channel: NetherNode stores it
encrypted, shows only its last few characters, and keeps it out of logs.
