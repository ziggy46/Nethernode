---
title: Security and threat model
description: What NetherNode protects, what it can't, and how to run it safely.
---

## What's exposed

| Part | Reachable by | Main risks |
| --- | --- | --- |
| Smart Proxy (TCP 25565) | The internet, by design | Malformed or flooding connections; players getting past bans |
| Web panel and API (8080) | Your network; optionally the internet through a tunnel | Password guessing, stolen sessions, cross-site attacks, a compromised account |
| Docker socket | NetherNode only | Whoever controls NetherNode controls Docker, which is root on the machine |
| Minecraft servers | Players, through the proxy | Server and mod vulnerabilities; they run in containers without extra privileges |
| Data folder | The machine's users | Worlds, backups, the database, and `secret.key` |

The most important fact: **an administrator account is effectively root on
the machine**, because NetherNode can start containers. Give accounts only
to people you'd trust with the machine, give everyone else the lowest role
that works, and turn on two-factor sign-in.

## The proxy

- Handshakes are parsed with strict size limits before anything else
  happens, and the parsers are fuzz-tested.
- Connection rates are limited per IP address.
- Bans (by name, UUID or IP, per server or everywhere) are enforced before a
  login reaches a server or wakes a sleeping one.
- Server ports are bound to loopback, so players can only reach servers
  through the proxy.

## The panel

- Passwords are hashed with Argon2id; sign-in is rate limited per address and
  per account, and wrong-password and unknown-account errors look the same.
- Optional two-factor sign-in (TOTP) with single-use recovery codes; a code
  can't be reused.
- Sessions are random tokens in `HttpOnly`, `SameSite=Lax` cookies (Secure
  over HTTPS), stored only as hashes, renewed with use, and ended after
  7 days idle or 30 days in all. New sessions on every sign-in; a password
  change signs out other devices.
- Every request that changes something needs a CSRF token.
- Responses carry a Content Security Policy that allows only NetherNode's own
  scripts (no inline scripts), plus `X-Frame-Options: DENY`,
  `X-Content-Type-Options: nosniff`, `Referrer-Policy`, and HSTS over HTTPS.
- Every API call checks the caller's role on the server; the panel hiding a
  button is never the only protection.
- File access is confined to each server's folder; archive extraction rejects
  path tricks, links, and zip bombs.
- Changes are recorded in the audit log, with who made them.

## Secrets

RCON passwords, management secrets, the CurseForge API key, the Discord
webhook URL and two-factor secrets are encrypted with `secret.key`
(ChaCha20-Poly1305). Anything NetherNode decrypts is also masked in its
logs, along with session cookies and password-like values.

## Recommendations

- Don't expose port 8080 to the internet. Use your network, a VPN, or a
  [Cloudflare Tunnel with Cloudflare Access](/guides/cloudflare-tunnel/) in
  front, so strangers never reach the sign-in page.
- Turn on two-factor sign-in, at least for owners and administrators.
- Back up the data folder, including `secret.key`, somewhere else. Without the
  key, stored secrets can't be read.
- Keep NetherNode, Docker, and the host updated. The server images are pulled
  fresh when servers are created.
- Review the audit log now and then, and remove accounts that aren't used.

## Reporting a problem

Please report security problems privately to the maintainers rather than in
a public issue, with steps to reproduce.
