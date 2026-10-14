/**
 * The schedule editor's "when" picker. Common choices map to cron
 * expressions and back; anything else is edited as cron directly.
 */

export type When =
  | { kind: "daily"; hour: number; minute: number }
  | { kind: "weekly"; day: number; hour: number; minute: number }
  | { kind: "hourly"; every: number; minute: number }
  | { kind: "custom"; cron: string };

export const WEEKDAYS = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];

/** Hour intervals that divide a day evenly, so runs land at the same times daily. */
export const HOUR_STEPS = [1, 2, 3, 4, 6, 8, 12];

export function toCron(when: When): string {
  switch (when.kind) {
    case "daily":
      return `${when.minute} ${when.hour} * * *`;
    case "weekly":
      return `${when.minute} ${when.hour} * * ${when.day}`;
    case "hourly":
      return when.every === 1 ? `${when.minute} * * * *` : `${when.minute} */${when.every} * * *`;
    case "custom":
      return when.cron.trim();
  }
}

const num = (s: string, max: number) => (/^\d{1,2}$/.test(s) && Number(s) <= max ? Number(s) : null);

/** Recognizes the expressions `toCron` makes; anything else is custom. */
export function fromCron(cron: string): When {
  const parts = cron.trim().split(/\s+/);
  const custom: When = { kind: "custom", cron: cron.trim() };
  if (parts.length !== 5) return custom;
  const [m, h, dom, mon, dow] = parts as [string, string, string, string, string];
  const minute = num(m, 59);
  if (minute === null || dom !== "*" || mon !== "*") return custom;
  const hour = num(h, 23);
  if (hour !== null && dow === "*") return { kind: "daily", hour, minute };
  const day = num(dow, 6);
  if (hour !== null && day !== null) return { kind: "weekly", day, hour, minute };
  if (dow !== "*") return custom;
  if (h === "*") return { kind: "hourly", every: 1, minute };
  const step = /^\*\/(\d{1,2})$/.exec(h);
  const every = step ? Number(step[1]) : null;
  if (every !== null && HOUR_STEPS.includes(every)) return { kind: "hourly", every, minute };
  return custom;
}

/** "04:00" for a time input. */
export function timeValue(hour: number, minute: number): string {
  return `${String(hour).padStart(2, "0")}:${String(minute).padStart(2, "0")}`;
}

/** Parses a time input's "HH:MM". */
export function parseTime(value: string): { hour: number; minute: number } | null {
  const match = /^(\d{2}):(\d{2})$/.exec(value);
  if (!match) return null;
  const hour = Number(match[1]);
  const minute = Number(match[2]);
  return hour < 24 && minute < 60 ? { hour, minute } : null;
}

/** The viewer's time zone, the default for new schedules. */
export function localTimeZone(): string {
  try {
    return Intl.DateTimeFormat().resolvedOptions().timeZone || "UTC";
  } catch {
    return "UTC";
  }
}

/** Every IANA zone the browser knows, with `current` included even if it doesn't. */
export function timeZones(current: string): string[] {
  let zones: string[] = [];
  try {
    zones = Intl.supportedValuesOf("timeZone");
  } catch {
    zones = [];
  }
  if (!zones.includes("UTC")) zones = ["UTC", ...zones];
  return zones.includes(current) ? zones : [current, ...zones];
}

/** A run time in the schedule's own zone: "Thu, Oct 15, 04:00". */
export function runTime(ms: number | bigint, timeZone: string): string {
  const options: Intl.DateTimeFormatOptions = {
    weekday: "short",
    month: "short",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit",
  };
  try {
    return new Date(Number(ms)).toLocaleString(undefined, { ...options, timeZone });
  } catch {
    return new Date(Number(ms)).toLocaleString(undefined, options);
  }
}
