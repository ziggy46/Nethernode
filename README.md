<p align="center"><img src="assets/icon-1024.png" width="128" alt="NetherNode icon"></p>

<h1 align="center">NetherNode</h1>

<p align="center">A fast, self-hosted Minecraft server manager.</p>

NetherNode is a single binary that runs a web panel, manages Minecraft servers
as Docker containers, and routes players to the right server by hostname
through one public port.

- **Servers:** Vanilla, Paper, Fabric, Forge, NeoForge and Quilt, plus
  Modrinth and CurseForge modpacks. Live console, file manager, resumable
  uploads of any size, mods, `server.properties` as a form.
- **Smart Proxy:** one port for every server, routed by hostname, with
  sleeping servers that wake when someone joins and bans enforced before a
  login reaches a server.
- **Players:** operators, allowlists, bans (per server or everywhere),
  inventories and statistics.
- **Backups and schedules:** live backups, safe restores, retention, and
  scheduled restarts, backups, commands and messages with in-game warnings.
- **People:** roles, one-time passwords, two-factor sign-in, an audit log,
  and Discord notifications.

> **Status:** 0.1, early. Expect rough edges and back up your worlds.

## Quick start

On a Linux machine with Docker:

```sh
cd deploy
docker compose up -d
```

Then open `http://<machine>:8080` and create the owner account. See
[the documentation](docs/site/src/content/docs/) for installing, DNS, the
proxy, Cloudflare Tunnel, backups, configuration, the API, and security.

## Development

You need Rust 1.96, Node 26 with pnpm, `protoc`, `just`, and Docker.

```sh
just setup     # install web dependencies
just dev       # panel with live reload (UI on :5173)
just lint      # formatting, clippy, protos, types, accessibility checks
just test      # Rust and web unit tests
just build     # release binary with the UI embedded
pnpm --dir web e2e          # browser tests against the built binary
pnpm --dir docs/site dev    # the documentation site
```

## License

[MIT](LICENSE)
