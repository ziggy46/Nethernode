/** "5 min ago"-style times. */
export function ago(ms: number | bigint, now: number): string {
  const then = Number(ms);
  if (!then) return "";
  const seconds = Math.max(0, Math.round((now - then) / 1000));
  if (seconds < 60) return "just now";
  const minutes = Math.round(seconds / 60);
  if (minutes < 60) return `${minutes} min ago`;
  const hours = Math.round(minutes / 60);
  if (hours < 24) return `${hours} h ago`;
  const days = Math.round(hours / 24);
  if (days < 30) return `${days} day${days === 1 ? "" : "s"} ago`;
  return new Date(then).toLocaleDateString();
}

/** "in 3 h 20 min"-style times still to come. */
export function until(ms: number | bigint, now: number): string {
  const left = Number(ms) - now;
  return left < 60_000 ? "in under a minute" : `in ${duration(left)}`;
}

/** "3 h 20 min"-style lengths of time. */
export function duration(ms: number | bigint): string {
  const minutes = Math.floor(Number(ms) / 60_000);
  if (minutes < 1) return "under a minute";
  if (minutes < 60) return `${minutes} min`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return minutes % 60 ? `${hours} h ${minutes % 60} min` : `${hours} h`;
  const days = Math.floor(hours / 24);
  return hours % 24 ? `${days} d ${hours % 24} h` : `${days} d`;
}

/** A date and time in the viewer's locale. */
export function dateTime(ms: number | bigint): string {
  return new Date(Number(ms)).toLocaleString(undefined, { dateStyle: "medium", timeStyle: "short" });
}
