---
title: The Smart Proxy
description: Listeners, routes, sleeping servers, and the PROXY protocol.
---

The Smart Proxy reads the hostname from each Minecraft handshake and passes
the connection to the right server. It answers server-list pings itself,
with the server's own MOTD, icon and player count when it's running, and a
clear message when it's asleep, starting, or stopped.

## Listeners

A listener is an address and port the proxy accepts players on, such as
`0.0.0.0:25565`. Add more on **Proxy & routes** to accept players on other
ports or addresses. Each can send a **PROXY protocol** header so servers
see players' real addresses; only turn that on for servers configured to
expect it, or every connection fails.

## Routes

Routes match the address a player typed. The most specific wins:

1. an exact hostname, like `play.example.com`;
2. a wildcard, like `*.example.com` (the longest matching suffix);
3. the default route `*`, for anything else, including connections by IP.

A wildcard without a fixed server picks the server whose slug equals the
subdomain. Routes can be limited to one listener. Each route card shows how
many players are connected through it right now.

## Sleeping servers

Servers set to sleep stop after a while with nobody online. The proxy keeps
answering pings for them, and the first player to join wakes the server;
they see a "starting" message and can join a few seconds later.

## Bans

Bans made in NetherNode, including bans across every server, are enforced by
the proxy before a login reaches a server or wakes it.
