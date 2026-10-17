import { queryOptions, useQuery } from "@tanstack/react-query";

import { type StatPeriod, StatsRange } from "@/gen/nethernode/v1/servers_pb";
import { serverClient } from "@/lib/api";
import { cn } from "@/lib/utils";

import { formatBytes } from "./format";
import { Sparkline } from "./sparkline";

/** The ranges the Performance panel offers. `HOUR` is the live view. */
export type Range = "hour" | "day" | "week" | "month";

export const RANGES: { id: Range; label: string; long: string; step: string }[] = [
  { id: "hour", label: "1 hour", long: "last 5 minutes", step: "" },
  { id: "day", label: "24 hours", long: "last 24 hours", step: "5 minutes" },
  { id: "week", label: "7 days", long: "last 7 days", step: "hour" },
  { id: "month", label: "30 days", long: "last 30 days", step: "4 hours" },
];

const PROTO: Record<Exclude<Range, "hour">, StatsRange> = {
  day: StatsRange.DAY,
  week: StatsRange.WEEK,
  month: StatsRange.MONTH,
};

const statsRangeQuery = (id: string, range: Exclude<Range, "hour">) =>
  queryOptions({
    queryKey: ["servers", "stats", id, range],
    queryFn: () => serverClient.getServerStats({ id, range: PROTO[range] }),
    staleTime: 60_000,
    refetchInterval: 60_000,
  });

/** Segmented control for the range. */
export function RangePicker({ value, onChange }: { value: Range; onChange: (range: Range) => void }) {
  return (
    <div role="tablist" aria-label="Time range" className="inline-flex rounded-md border border-border p-0.5">
      {RANGES.map((r) => (
        <button
          key={r.id}
          type="button"
          role="tab"
          aria-selected={value === r.id}
          onClick={() => onChange(r.id)}
          className={cn(
            "rounded-sm px-2.5 py-1 text-meta",
            value === r.id
              ? "bg-nav-active text-nav-active-foreground"
              : "text-text-muted hover:text-text-primary",
          )}
        >
          {r.label}
        </button>
      ))}
    </div>
  );
}

/**
 * One slot per period from the start of the range to now, `undefined`
 * where the server wasn't running.
 */
function timeline(periods: StatPeriod[], since: number): (StatPeriod | undefined)[] {
  const first = periods[0];
  if (!first) return [];
  const step = first.seconds * 1000;
  const byStart = new Map(periods.map((p) => [Number(p.atMs), p]));
  const slots = Math.max(1, Math.floor((Date.now() - since) / step) + 1);
  return Array.from({ length: slots }, (_, i) => byStart.get(since + i * step));
}

/** Weighted by running time, so a few minutes don't count like a full period. */
function average(periods: StatPeriod[], value: (p: StatPeriod) => number | undefined): number | undefined {
  let sum = 0;
  let weight = 0;
  for (const p of periods) {
    const v = value(p);
    if (v === undefined) continue;
    sum += v * p.runningSeconds;
    weight += p.runningSeconds;
  }
  return weight > 0 ? sum / weight : undefined;
}

function hours(seconds: number): string {
  if (seconds < 3600) return `${Math.max(1, Math.round(seconds / 60))} min`;
  const h = seconds / 3600;
  return `${h < 10 ? h.toFixed(1) : Math.round(h)} h`;
}

function bytes(n: number): string {
  return n < 1024 * 1024 ? `${Math.ceil(n / 1024)} KB` : formatBytes(n);
}

/** The server's stored history over the day, week, or month. */
export function HistoryTiles({
  serverId,
  range,
  playersMax,
  children,
}: {
  serverId: string;
  range: Exclude<Range, "hour">;
  playersMax?: number;
  /** Tiles that don't depend on the range (storage). */
  children?: React.ReactNode;
}) {
  const data = useQuery(statsRangeQuery(serverId, range)).data;
  const meta = RANGES.find((r) => r.id === range);
  if (!data) {
    return <div className="h-64 animate-pulse rounded-md bg-surface" role="status" aria-busy />;
  }
  const periods = data.periods;
  const slots = timeline(periods, Number(data.sinceMs));
  const label = meta?.long ?? "";
  if (periods.length === 0) {
    return (
      <div className="grid grid-cols-2 gap-6 below-sm:grid-cols-1">
        <p className="col-span-2 text-label text-text-muted below-sm:col-span-1">
          Nothing recorded in the {label} yet. The panel keeps a summary of each minute while the server runs.
        </p>
        {children}
      </div>
    );
  }

  const running = periods.reduce((s, p) => s + p.runningSeconds, 0);
  const paused = periods.reduce((s, p) => s + p.pausedSeconds, 0);
  const cpu = average(periods, (p) => p.cpuAvg) ?? 0;
  const cpuPeak = Math.max(...periods.map((p) => p.cpuMax));
  const memory = average(periods, (p) => Number(p.memoryAvg)) ?? 0;
  const memoryPeak = Math.max(...periods.map((p) => Number(p.memoryMax)));
  const limit = Number(periods[periods.length - 1]?.memoryLimit ?? 0);
  const tps = average(periods, (p) => p.tpsAvg);
  const tpsLow = Math.min(...periods.flatMap((p) => (p.tpsMin === undefined ? [] : [p.tpsMin])));
  const msptPeak = Math.max(0, ...periods.flatMap((p) => (p.msptMax === undefined ? [] : [p.msptMax])));
  const players = Math.max(0, ...periods.map((p) => p.playersMax ?? 0));
  const written = periods.reduce((s, p) => s + Number(p.diskWrite), 0);
  const read = periods.reduce((s, p) => s + Number(p.diskRead), 0);
  const perSecond = (p: StatPeriod | undefined, n: bigint | undefined) =>
    p && n !== undefined ? Number(n) / Math.max(1, p.runningSeconds) : null;

  return (
    <>
      <p className="mb-5 text-meta text-text-muted">
        Running for {hours(running)} of the {label}. Each point covers {meta?.step}; gaps are when the server
        wasn't running.
      </p>
      <div className="grid grid-cols-2 gap-x-6 gap-y-7 below-sm:grid-cols-1">
        <Tile
          label={`CPU · ${label}`}
          value={`${Math.round(cpu)}%`}
          unit="on average"
          note={`Peak ${Math.round(cpuPeak)}%`}
        >
          <Sparkline values={slots.map((p) => p?.cpuAvg ?? null)} max={100} className="mt-2 h-16 w-full" />
        </Tile>
        <Tile
          label={`Memory · ${label}`}
          value={formatBytes(memory)}
          unit={`on average, of ${formatBytes(limit)}`}
          note={`Peak ${formatBytes(memoryPeak)}`}
        >
          <Sparkline
            values={slots.map((p) => (p ? Number(p.memoryAvg) : null))}
            max={limit}
            className="mt-2 h-16 w-full"
          />
        </Tile>
        <Tile
          label={`Tick health · ${label}`}
          value={tps !== undefined ? tps.toFixed(1) : "—"}
          unit={tps !== undefined ? "TPS on average" : undefined}
          tone={tps === undefined ? undefined : tpsLow >= 19.5 ? "good" : tpsLow >= 15 ? "warn" : "bad"}
          note={
            tps === undefined
              ? "The server didn't report tick times. Some older or modded servers don't."
              : `Lowest ${tpsLow.toFixed(1)} TPS; slowest moment ${msptPeak.toFixed(0)} ms per tick (50 ms keeps up).${
                  paused > 0 ? ` Paused for ${hours(paused)} while nobody was online.` : ""
                }`
          }
        >
          {tps !== undefined && (
            <Sparkline values={slots.map((p) => p?.msptAvg ?? null)} max={50} className="mt-2 h-16 w-full" />
          )}
        </Tile>
        <Tile label={`Players · ${label}`} value={String(players)} unit="most at once">
          <Sparkline
            values={slots.map((p) => (p ? (p.playersMax ?? 0) : null))}
            max={Math.max(1, playersMax ?? 0)}
            className="mt-2 h-16 w-full"
          />
        </Tile>
        <Tile label={`Disk · ${label}`} value={bytes(written)} unit="written" note={`${bytes(read)} read`}>
          <Sparkline
            values={slots.map((p) => perSecond(p, p?.diskWrite))}
            max={1}
            className="mt-2 h-16 w-full"
          />
        </Tile>
        {children}
      </div>
    </>
  );
}

export function Tile({
  label,
  value,
  unit,
  note,
  tone,
  children,
}: {
  label: string;
  value: string;
  unit?: string;
  note?: string;
  tone?: "good" | "warn" | "bad";
  children?: React.ReactNode;
}) {
  return (
    <div className="min-w-0">
      <p className="text-meta text-text-muted">{label}</p>
      <p
        className={cn(
          "font-[650] text-title tabular-nums",
          tone === "good" && "text-success",
          tone === "warn" && "text-warning",
          tone === "bad" && "text-danger",
        )}
      >
        {value}
        {unit && <span className="ml-1 font-normal text-label text-text-muted">{unit}</span>}
      </p>
      {note && <p className="mt-0.5 text-meta text-text-muted">{note}</p>}
      {children}
    </div>
  );
}
