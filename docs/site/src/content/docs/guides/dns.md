---
title: DNS and ports
description: Point a domain at NetherNode and let players in.
---

Players connect to the Smart Proxy (port 25565 by default), which reads the
address they typed and sends them to the matching server. For that to work
from the internet you need DNS records and, at home, a port forward.

## Records

The **Proxy & routes** page lists the records your routes need, under
**DNS records to create**. In general:

| Route | Record |
| --- | --- |
| `play.example.com` | `A` (and `AAAA` for IPv6) `play.example.com` → your public IP |
| `*.mc.example.com` | `A` `*.mc.example.com` → your public IP |
| anything, proxy not on 25565 | also `SRV` `_minecraft._tcp.play.example.com` → `0 5 <port> play.example.com` |

A wildcard route such as `*.mc.example.com` with no fixed server sends
`smp.mc.example.com` to the server whose short name (slug) is `smp`, so new
servers are reachable without new records.

Use each route's **DNS** button to check what a name resolves to.

:::note[Cloudflare]
Minecraft records must be **DNS only** (grey cloud). Cloudflare's proxy
carries web traffic, not Minecraft.
:::

## Ports

- Forward **TCP 25565** on your router to the NetherNode machine. That's the
  only port players need; servers' own ports stay on loopback.
- Don't forward the panel's port (8080). Reach it on your network, over a
  VPN, or through a [Cloudflare Tunnel](/guides/cloudflare-tunnel/).
- If your provider uses CGNAT (no public IPv4), port forwarding won't work;
  use IPv6 or a TCP tunnelling service.

## Checking it works

From outside your network, add the server in Minecraft's multiplayer menu
with the hostname. If the server is asleep, the list shows it as starting
and joining wakes it.
