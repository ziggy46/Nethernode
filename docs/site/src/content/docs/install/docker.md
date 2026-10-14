---
title: Install with Docker Compose
description: The recommended way to run NetherNode.
---

## What you need

- A Linux machine (x86-64 or ARM64) with Docker Engine and the Compose
  plugin. A VM, a Proxmox container with Docker, or a spare PC all work.
- Memory for your servers: about 2 GB per small Vanilla or Paper server,
  6 GB or more for large modpacks. NetherNode itself uses very little.
- Port 25565 (TCP) reachable from the internet, if people outside your
  network will play. See [DNS and ports](/guides/dns/).

## Start it

Get `deploy/docker-compose.yml` from the repository, then:

```sh
cd deploy
docker compose up -d
docker compose logs -f   # optional: watch it start
```

From a checkout, the first `up` builds NetherNode's image (a few minutes).
To use a released image instead, set
`NETHERNODE_IMAGE=ghcr.io/<owner>/nethernode:latest` in `.env`.

Open `http://<machine>:8080` and create the owner account. The first server
takes a minute or two: Docker downloads the Minecraft server image, then the
server downloads its own files.

## Settings

Put overrides in a `.env` file next to `docker-compose.yml`:

| Variable | Default | Meaning |
| --- | --- | --- |
| `NETHERNODE_DATA` | `./data` | Folder (or named volume) for worlds, backups and settings |
| `NETHERNODE_BIND` | `0.0.0.0:8080` | The panel's address |
| `NETHERNODE_PROXY` | `0.0.0.0:25565` | Where players connect (first run only; then edit listeners in the panel) |
| `NETHERNODE_PORTS_START` / `_END` | `30000` / `30999` | Host ports for servers, on loopback only |

Every [configuration setting](/reference/configuration/) can also be set
with an environment variable in the compose file.

## How the data folder works

NetherNode mounts each server's folder into that server's container. Docker
needs the folder's path on the host for that, which differs from `/data`
inside NetherNode's container. NetherNode works it out by inspecting its own
container, so the data folder can be any host directory or named volume. If
it can't (for example when the Docker socket is behind a filtering proxy),
the log says so, and you can set it yourself:

```yaml
environment:
  NETHERNODE_DOCKER__HOST_DATA_DIR: /srv/nethernode/data
```

## Host or bridge networking

The compose file uses **host networking**: the panel and the proxy listen
directly on the machine's addresses, and servers' ports stay on `127.0.0.1`
where only NetherNode can reach them. That's the simplest setup.

Where host networking isn't available (Docker Desktop), use bridge
networking: remove `network_mode: host`, publish the ports, and tell
NetherNode to reach its servers through the Docker bridge gateway:

```yaml
ports:
  - "8080:8080"
  - "25565:25565"
environment:
  NETHERNODE_DOCKER__BACKEND_IP: "172.17.0.1"   # docker network inspect bridge
```

In bridge mode NetherNode sees every player as coming from the gateway
address, so per-IP bans and limits apply to everyone at once. Prefer host
networking where you can.

## Updating

```sh
docker compose pull   # or: docker compose build, from a checkout
docker compose up -d
```

Servers keep running while NetherNode restarts; it reconnects to them.
Back up the data folder before major updates.
