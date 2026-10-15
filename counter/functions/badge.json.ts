/** GET /badge.json: a shields.io endpoint with installs active in 30 days. */
import { ACTIVE_DAYS, badge, dayNumber } from "../src/counter";
import type { Env } from "./_shared";

export const onRequestGet: PagesFunction<Env> = async ({ env }) => {
  const row = await env.DB.prepare("SELECT COUNT(*) AS n FROM installs WHERE last_day > ?1")
    .bind(dayNumber(Date.now()) - ACTIVE_DAYS)
    .first<{ n: number }>();
  return Response.json(badge(row?.n ?? 0), {
    headers: { "cache-control": "public, max-age=3600" },
  });
};
