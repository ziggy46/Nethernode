# NetherNode install counter

A Cloudflare Worker behind the "active installs" badge. NetherNode installs
whose owners turn on **Count this install** send a random install ID and their
version once a day; this counts the IDs heard from in the last 30 days. It
stores only the ID, the version, and the day of the last report, forgets an
install after 90 days of silence, and keeps no request logs.

## Deploy (once)

From this folder, logged in to the Cloudflare account that owns the
`workers.dev` subdomain:

```sh
pnpm install
export WRANGLER_SEND_METRICS=false
pnpm exec wrangler login
pnpm exec wrangler d1 create nethernode-installs
# copy the printed database_id into wrangler.toml
pnpm exec wrangler d1 migrations apply nethernode-installs --remote
pnpm exec wrangler deploy
```

It answers at `https://nethernode-installs.<subdomain>.workers.dev`:

- `POST /ping` with `{"id": "<uuid v4>", "version": "x.y.z"}`
- `GET /badge.json`, a [shields.io endpoint](https://shields.io/badges/endpoint-badge)

## Develop

```sh
pnpm test
pnpm exec wrangler d1 migrations apply nethernode-installs --local
pnpm dev
```
