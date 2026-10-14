---
title: API
description: The ConnectRPC API the panel uses, and how to call it.
---

Everything the panel does goes through NetherNode's API, so anything in the
panel can be scripted. The API is [ConnectRPC](https://connectrpc.com):
each call is an HTTP `POST` of JSON to
`/api/rpc/nethernode.v1.<Service>/<Method>` (calls without side effects
also accept `GET`). The definitions, with comments, are the `.proto` files
in `proto/nethernode/v1/` in the repository; any ConnectRPC or gRPC-web
client library can generate a typed client from them.

| Service | For |
| --- | --- |
| `AuthService` | First-run setup, sign-in (including the two-factor step), sign-out |
| `AccountService` | Your password, two-factor sign-in, signed-in devices |
| `UserService` | Accounts and roles (owners) |
| `ServerService` | Creating, starting, stopping and configuring servers; host information |
| `FileService` | Server files |
| `PlayerService` | Players, bans, operators, the allowlist |
| `CatalogService`, `ModService` | Versions, modpacks, mods |
| `BackupService`, `ScheduleService` | Backups, restores, schedules |
| `ProxyService` | Listeners, routes, live connections |
| `AuditService` | The audit log and activity feed |
| `NotificationService` | Discord notifications |

## Signing in from a script

The API uses the same session cookie as the browser, plus a CSRF token on
every call that changes something:

```sh
base=http://127.0.0.1:8080
jar=$(mktemp)
curl -s -c "$jar" -o /dev/null "$base/healthz"            # gets the nn_csrf cookie
csrf=$(awk '$6 == "nn_csrf" { print $7 }' "$jar")
rpc() {
  curl -s -b "$jar" -c "$jar" -H "content-type: application/json" \
    -H "x-csrf-token: $csrf" -d "$2" "$base/api/rpc/nethernode.v1.$1"
}
rpc AuthService/Login '{"username":"me","password":"…"}'
rpc ServerService/ListServers '{}'
```

If the account uses two-factor sign-in, `Login` returns a
`secondFactorChallenge`; send it with a code to `AuthService/FinishSignIn`.
Consider a separate account with only the role the script needs.

## Other endpoints

| Path | What |
| --- | --- |
| `GET /healthz` | `{"status":"ok","version":"…"}`, no sign-in needed |
| `GET /metrics` | Prometheus metrics (signed in as an owner) |
| `GET /api/events` | Server-sent events: status changes, player joins, job progress |
| `/ws/servers/{id}/console` | The live console (WebSocket) |
| `/api/uploads` | Resumable uploads ([tus](https://tus.io) 1.0) |
