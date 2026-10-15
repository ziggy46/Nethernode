# NetherNode install counter

The service behind the "active installs" badge, a Cloudflare Pages project
with Functions at `https://nethernode-installs.pages.dev`. NetherNode installs
whose owners turn on **Count this install** send a random install ID and their
version once a day; this counts the IDs heard from in the last 30 days. It
stores only the ID, the version, and the day of the last report, forgets an
install after 90 days of silence, and keeps no request logs.

- `POST /ping` with `{"id": "<uuid v4>", "version": "x.y.z"}`
- `GET /badge.json`, a [shields.io endpoint](https://shields.io/badges/endpoint-badge)
- `GET /`, a page saying what's collected

Anyone can send made-up IDs, so the count can be inflated; it's an
indication, not a measurement. If that ever matters, a Cloudflare rate
limiting rule on `/ping` is the fix.

## Deploy

From this folder, logged in to the Cloudflare account that owns the Pages
project:

```sh
pnpm install
export WRANGLER_SEND_METRICS=false
pnpm exec wrangler login
pnpm exec wrangler d1 create nethernode-installs
# copy the printed database_id into wrangler.toml
pnpm exec wrangler d1 migrations apply nethernode-installs --remote
pnpm run deploy
```

Later deploys need only the last command.

## Develop

```sh
pnpm test
pnpm exec wrangler d1 migrations apply nethernode-installs --local
pnpm dev
```
