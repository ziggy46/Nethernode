/**
 * What the counter accepts and how it counts. Kept free of Worker APIs so
 * it can be tested on its own.
 */

/** Installs that reported within this many days are "active". */
export const ACTIVE_DAYS = 30;
/** Installs silent for this long are forgotten entirely. */
export const FORGET_DAYS = 90;

const DAY_MS = 24 * 60 * 60 * 1000;

/** Days since the Unix epoch, in UTC. Only the day is ever stored. */
export function dayNumber(ms: number): number {
  return Math.floor(ms / DAY_MS);
}

/** A random version-4 UUID, as NetherNode makes for each install. */
export function isInstallId(value: unknown): value is string {
  return typeof value === "string" && /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/.test(value);
}

/** A version like `0.1.0` or `1.2.0-rc.1`. */
export function isVersion(value: unknown): value is string {
  return (
    typeof value === "string" && value.length <= 40 && /^\d{1,4}\.\d{1,4}\.\d{1,4}(?:-[0-9A-Za-z.]{1,24})?$/.test(value)
  );
}

export interface Ping {
  id: string;
  version: string;
}

/** The report, if `body` is exactly one: `{"id": …, "version": …}`. */
export function parsePing(body: string): Ping | null {
  if (body.length > 200) return null;
  let parsed: unknown;
  try {
    parsed = JSON.parse(body);
  } catch {
    return null;
  }
  if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) return null;
  const { id, version, ...rest } = parsed as Record<string, unknown>;
  if (Object.keys(rest).length > 0 || !isInstallId(id) || !isVersion(version)) return null;
  return { id, version };
}

/** A shields.io endpoint badge. */
export function badge(active: number) {
  return { schemaVersion: 1, label: "active installs", message: String(active), color: "f2701c" };
}
