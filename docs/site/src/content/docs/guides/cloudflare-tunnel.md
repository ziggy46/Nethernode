---
title: Cloudflare Tunnel
description: Reach the panel from anywhere without opening a port.
---

A Cloudflare Tunnel publishes the web panel on a hostname such as
`panel.example.com` with no port forward. It carries HTTP, so it can't
carry Minecraft: players still connect to the Smart Proxy directly (see
[DNS and ports](/guides/dns/)).

## Set it up

```sh
cloudflared tunnel login
cloudflared tunnel create nethernode
cloudflared tunnel route dns nethernode panel.example.com
```

Then run it with a config like `deploy/cloudflared.example.yml`:

```yaml
tunnel: nethernode
credentials-file: /etc/cloudflared/nethernode.json
ingress:
  - hostname: panel.example.com
    service: http://127.0.0.1:8080
  - service: http_status:404
```

Tell NetherNode it's behind a proxy, so it records visitors' real addresses
and marks cookies secure:

```sh
NETHERNODE_HTTP__TRUST_PROXY_HEADERS=true
```

## Put Cloudflare Access in front

Strongly recommended: in Cloudflare Zero Trust, add an **Access application**
for `panel.example.com` that only lets in the people you choose (by email,
for example). Strangers then never reach NetherNode's sign-in page at all,
which also keeps password guessing away from it. Two-factor sign-in in
NetherNode is still worth turning on.

Large uploads work through the tunnel: they're sent in pieces small enough
for Cloudflare's limits and resume if the connection drops.
