---
title: Run the binary
description: Install NetherNode without a container, under systemd.
---

NetherNode is a single static binary with the web panel built in. Running it
directly is useful when you'd rather not put the Docker socket in a
container.

1. Download `nethernode-<version>-linux-amd64.tar.gz` (or `arm64`) from the
   releases page, check it against `SHA256SUMS`, and unpack it:

   ```sh
   sha256sum -c --ignore-missing SHA256SUMS
   tar -xzf nethernode-*-linux-amd64.tar.gz
   sudo install -m 0755 nethernode /usr/local/bin/
   ```

2. Create a user that may use Docker, and a data folder:

   ```sh
   sudo useradd --system --home /var/lib/nethernode --create-home nethernode
   sudo usermod -aG docker nethernode
   ```

3. Add `/etc/systemd/system/nethernode.service`:

   ```ini
   [Unit]
   Description=NetherNode
   After=docker.service network-online.target
   Requires=docker.service

   [Service]
   User=nethernode
   WorkingDirectory=/var/lib/nethernode
   Environment=NETHERNODE_DATA__DIR=/var/lib/nethernode/data
   ExecStart=/usr/local/bin/nethernode serve
   Restart=on-failure

   [Install]
   WantedBy=multi-user.target
   ```

4. Start it:

   ```sh
   sudo systemctl daemon-reload
   sudo systemctl enable --now nethernode
   journalctl -u nethernode -f
   ```

Servers run as the same user ID as NetherNode, so it can always manage their
files.

## Commands

| Command | What it does |
| --- | --- |
| `nethernode serve` | Runs the panel, API and proxy (the default) |
| `nethernode migrate` | Updates the database and exits |
| `nethernode reset-admin [--username NAME]` | Gives an owner a new one-time password, turns off their two-factor sign-in, and signs them out everywhere |
| `nethernode version` | Prints the version |

`reset-admin` is the way back in if an owner loses their password, phone and
recovery codes. It needs access to the machine, which is the point.
