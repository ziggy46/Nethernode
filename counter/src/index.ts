/**
 * Counts NetherNode installs whose owners chose to report in. Each report
 * is a random install ID and a version; this stores only those and the day
 * of the last report. No IP addresses or anything else are kept.
 *
 *   POST /ping         {"id": "<uuid>", "version": "0.1.0"}  → 204
 *   GET  /badge.json   shields.io endpoint: installs active in 30 days
 */
import { ACTIVE_DAYS, badge, dayNumber, FORGET_DAYS, parsePing } from "./counter";

export interface Env {
  DB: D1Database;
}

const ABOUT = `NetherNode install counter.

NetherNode installs can opt in to reporting a random install ID and their
version once a day. This counts how many reported in the last ${ACTIVE_DAYS} days.
Only the ID, the version, and the day of the last report are stored; reports
are forgotten after ${FORGET_DAYS} days of silence.
`;

export default {
  async fetch(request, env, ctx): Promise<Response> {
    const url = new URL(request.url);
    const today = dayNumber(Date.now());

    if (url.pathname === "/ping") {
      if (request.method !== "POST") return new Response("Use POST.", { status: 405, headers: { allow: "POST" } });
      const ping = parsePing(await request.text());
      if (!ping) return new Response("Expected {\"id\": <uuid v4>, \"version\": <x.y.z>}.", { status: 400 });
      await env.DB.prepare(
        `INSERT INTO installs (id, version, last_day) VALUES (?1, ?2, ?3)
         ON CONFLICT (id) DO UPDATE SET version = excluded.version, last_day = excluded.last_day`,
      )
        .bind(ping.id, ping.version, today)
        .run();
      return new Response(null, { status: 204 });
    }

    if (url.pathname === "/badge.json" && request.method === "GET") {
      const cache = caches.default;
      const cached = await cache.match(request);
      if (cached) return cached;
      const row = await env.DB.prepare("SELECT COUNT(*) AS n FROM installs WHERE last_day > ?1")
        .bind(today - ACTIVE_DAYS)
        .first<{ n: number }>();
      const response = Response.json(badge(row?.n ?? 0), {
        headers: { "cache-control": "public, max-age=3600" },
      });
      ctx.waitUntil(cache.put(request, response.clone()));
      return response;
    }

    if (url.pathname === "/" && request.method === "GET") {
      return new Response(ABOUT, { headers: { "content-type": "text/plain; charset=utf-8" } });
    }
    return new Response("Not found.", { status: 404 });
  },

  async scheduled(_controller, env): Promise<void> {
    await env.DB.prepare("DELETE FROM installs WHERE last_day < ?1")
      .bind(dayNumber(Date.now()) - FORGET_DAYS)
      .run();
  },
} satisfies ExportedHandler<Env>;
