/**
 * POST /ping  {"id": "<uuid v4>", "version": "x.y.z"}  → 204
 *
 * Records that an install reported today. Only the ID, the version, and the
 * day are stored; installs silent for 90 days are forgotten here too, since
 * Pages has no scheduled jobs.
 */
import { dayNumber, FORGET_DAYS, parsePing } from "../src/counter";
import type { Env } from "./_shared";

export const onRequestPost: PagesFunction<Env> = async ({ request, env, waitUntil }) => {
  const ping = parsePing(await request.text());
  if (!ping) return new Response('Expected {"id": <uuid v4>, "version": <x.y.z>}.', { status: 400 });
  const today = dayNumber(Date.now());
  await env.DB.prepare(
    `INSERT INTO installs (id, version, last_day) VALUES (?1, ?2, ?3)
     ON CONFLICT (id) DO UPDATE SET version = excluded.version, last_day = excluded.last_day`,
  )
    .bind(ping.id, ping.version, today)
    .run();
  waitUntil(env.DB.prepare("DELETE FROM installs WHERE last_day < ?1").bind(today - FORGET_DAYS).run());
  return new Response(null, { status: 204 });
};

export const onRequest: PagesFunction<Env> = async () =>
  new Response("Use POST.", { status: 405, headers: { allow: "POST" } });
