---
title: The install count
description: The opt-in count of active installs, and exactly what it sends.
---

The NetherNode README shows how many installs are active. That number only
includes installs whose owners chose to be counted. **It's off unless you
turn it on.**

## Turning it on or off

- In the panel: **Settings › Count this install** (owners only).
- Or in the configuration: `install_count.enabled = true`, or
  `NETHERNODE_COUNT_INSTALL=true` in the Docker Compose `.env` file. When
  the configuration turns it on, the Settings switch shows it and can't turn
  it off.

Turning it off deletes the install ID, so turning it back on later counts as
a new install.

## What's sent

About once a day (and shortly after you turn it on), NetherNode sends one
HTTPS request to the counter with exactly this, which Settings also shows:

```json
{"id": "3f1c2b9a-7d4e-4a6b-9c2d-1e0f5a7b8c9d", "version": "0.1.0"}
```

- `id` is a random UUID made on your machine when you turn counting on. It
  isn't derived from anything: not your hardware, hostname, account or
  address.
- `version` is NetherNode's version.

Nothing else is sent: no names, hostnames, server or player information,
settings, or usage.

## What the counter keeps

The counter is a small open-source Cloudflare Pages project
(`counter/` in the repository) at `https://nethernode-installs.pages.dev`. It stores each install ID, its version, and the day of the last
report, and forgets an install after 90 days without one. It keeps no
request logs and doesn't store IP addresses, though like any web request,
the connection itself comes from your address. The badge counts IDs heard
from in the last 30 days.
