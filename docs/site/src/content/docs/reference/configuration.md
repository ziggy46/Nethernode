---
title: Configuration
description: Every setting, its default, and its environment variable.
---

NetherNode reads `nethernode.toml` from the working directory (another path
with `--config` or `NETHERNODE_CONFIG`), then environment variables, which
win. A missing file is fine: everything has a default. An environment
variable is `NETHERNODE_` plus the section and key, joined by `__`, such as
`NETHERNODE_HTTP__BIND`.

```toml
[http]
bind = "0.0.0.0:8080"
secure_cookies = "auto"
trust_proxy_headers = false

[data]
dir = "./data"

[docker]
socket = "unix:///var/run/docker.sock"
port_range_start = 30000
port_range_end = 30999
memory_overhead_percent = 25
memory_overhead_min_mb = 1024
backend_ip = "127.0.0.1"
# host_data_dir = "/srv/nethernode/data"

[proxy]
enabled = true
default_listener = "0.0.0.0:25565"

[uploads]
chunk_limit_mb = 16
max_file_gb = 50
quota_gb = 100
expiry_hours = 24

[install_count]
enabled = false
```

## `[http]`

| Key | Default | Meaning |
| --- | --- | --- |
| `bind` | `0.0.0.0:8080` | Address of the panel and API |
| `secure_cookies` | `auto` | `auto` marks cookies Secure when the request arrived over HTTPS (directly or per `X-Forwarded-Proto`); `always` or `never` force it. HTTPS requests also get `Strict-Transport-Security`. |
| `trust_proxy_headers` | `false` | Take visitors' addresses from `CF-Connecting-IP` / `X-Real-IP` / `X-Forwarded-For`. Turn on only behind a reverse proxy or tunnel, or anyone could fake their address. |

## `[data]`

| Key | Default | Meaning |
| --- | --- | --- |
| `dir` | `./data` | Database (`nethernode.db`), encryption key (`secret.key`), `servers/`, `backups/`, caches. Back it up; keep `secret.key` private: it decrypts stored RCON passwords, API keys and two-factor secrets. |

## `[docker]`

| Key | Default | Meaning |
| --- | --- | --- |
| `socket` | `unix:///var/run/docker.sock` | Docker Engine endpoint |
| `port_range_start`, `port_range_end` | `30000`, `30999` | Host ports given to servers (game, RCON, management), bound to `backend_ip` |
| `memory_overhead_percent`, `memory_overhead_min_mb` | `25`, `1024` | Container memory beyond the Java heap: the larger of the two. Modded servers get at least 40% / 2048 MB. |
| `backend_ip` | `127.0.0.1` | Where servers' ports are published and where NetherNode connects to them. Change only for bridge networking (the Docker gateway, usually `172.17.0.1`). |
| `host_data_dir` | unset | The data folder's path on the Docker host, when NetherNode runs in a container and can't find it itself |

## `[proxy]`

| Key | Default | Meaning |
| --- | --- | --- |
| `enabled` | `true` | Run the Smart Proxy |
| `default_listener` | `0.0.0.0:25565` | Listener created on first run; manage listeners in the panel afterwards |

## `[uploads]`

| Key | Default | Meaning |
| --- | --- | --- |
| `chunk_limit_mb` | `16` | Largest piece of an upload per request (1–64). Keep it under Cloudflare's 100 MB request limit. |
| `max_file_gb` | `50` | Largest single upload |
| `quota_gb` | `100` | Most unfinished-upload data per person at once |
| `expiry_hours` | `24` | Unfinished uploads with no progress for this long are discarded |

## `[install_count]`

| Key | Default | Meaning |
| --- | --- | --- |
| `enabled` | `false` | Add this install to the public count of active installs. Owners can also turn it on in Settings; when the configuration turns it on, Settings can't turn it off. See [what's sent](/reference/install-count/). |
| `url` | the NetherNode counter | Where the daily report goes |

## Logging

`RUST_LOG` sets the log level, for example `RUST_LOG=debug` or
`RUST_LOG=info,nn_proxy=debug`. Secrets NetherNode knows about (RCON
passwords, API keys, webhook URLs, session cookies) are masked in logs.
