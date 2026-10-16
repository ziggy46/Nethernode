import { useQuery } from "@tanstack/react-query";
import { Zap } from "lucide-react";
import type * as React from "react";

import { Panel, PanelHeader } from "@/components/page";
import { type Server, ServerStatus } from "@/gen/nethernode/v1/servers_pb";
import { cn } from "@/lib/utils";

import { formatBytes } from "./format";
import { statsHistoryQuery } from "./queries";
import { useLiveStats } from "./server-card";
import { type Point, Sparkline, useStatsSeries } from "./sparkline";

/** Samples in the short charts (one a second). */
const RECENT = 300;
/** Points the hour-long players chart is thinned to. */
const HOUR_POINTS = 240;

/** The server's resources, tick health, players, and disk. */
export function PerformancePanel({ server }: { server: Server }) {
  const running = server.status === ServerStatus.RUNNING;
  const series = useStatsSeries(server.id, running || server.status === ServerStatus.STARTING);
  const details = useQuery({ ...statsHistoryQuery(server.id), refetchInterval: 5 * 60_000 }).data;
  const live = useLiveStats(server.id);
  const recent = series.slice(-RECENT);
  const graphs = running && recent.length > 1;

  const tick = [...recent].reverse().find((p) => p.mspt !== undefined);
  const players = live?.players ?? server.playersOnline;
  const writes = rates(recent, "diskWrite");
  const reads = rates(recent, "diskRead");

  return (
    <Panel>
      <PanelHeader title="Performance" icon={<Zap />} />
      {graphs ? (
        <div className="grid grid-cols-2 gap-x-6 gap-y-7 below-sm:grid-cols-1">
          <Tile label="CPU · last 5 minutes" value={`${Math.round(live?.cpuPercent ?? 0)}%`}>
            <Sparkline values={recent.map((p) => p.cpu)} max={100} className="mt-2 h-16 w-full" />
          </Tile>
          <Tile
            label="Memory · last 5 minutes"
            value={formatBytes(live?.memoryBytes ?? 0)}
            unit={`/ ${formatBytes(live?.memoryLimitBytes ?? 0)}`}
          >
            <Sparkline
              values={recent.map((p) => p.memory)}
              max={recent[recent.length - 1]?.limit ?? 0}
              className="mt-2 h-16 w-full"
            />
          </Tile>
          <Tile
            label="Tick health · last 5 minutes"
            value={tick?.tps !== undefined ? tick.tps.toFixed(1) : "—"}
            unit={tick ? "TPS" : undefined}
            tone={
              tick?.tps === undefined
                ? undefined
                : tick.tps >= 19.5
                  ? "good"
                  : tick.tps >= 15
                    ? "warn"
                    : "bad"
            }
            note={
              tick?.mspt !== undefined
                ? `${tick.mspt.toFixed(1)} ms per tick on average${
                    tick.msptP95 !== undefined ? `, ${tick.msptP95.toFixed(1)} ms for the slowest 5%` : ""
                  } (50 ms keeps up)`
                : "Waiting for the server to report tick times. Some older or modded servers don't."
            }
          >
            {tick && (
              <Sparkline values={recent.map((p) => p.mspt ?? 0)} max={50} className="mt-2 h-16 w-full" />
            )}
          </Tile>
          <Tile
            label="Players · last hour"
            value={players !== undefined ? String(players) : "—"}
            unit={server.playersMax ? `/ ${server.playersMax}` : undefined}
            note={`Most at once in the last 24 hours: ${details?.playersPeakDay ?? 0}`}
          >
            <Sparkline
              values={thin(series, HOUR_POINTS).map((p) => p.players ?? 0)}
              max={Math.max(1, server.playersMax ?? 0)}
              className="mt-2 h-16 w-full"
            />
          </Tile>
          <Tile
            label="Disk · last 5 minutes"
            value={`${perSecond(last(writes))}`}
            unit="written"
            note={`${perSecond(last(reads))} read`}
          >
            <Sparkline values={writes} max={Math.max(1, ...writes)} className="mt-2 h-16 w-full" />
          </Tile>
          <Storage folder={details?.folderBytes} world={details?.worldBytes} />
        </div>
      ) : (
        <div className="grid grid-cols-2 gap-6 below-sm:grid-cols-1">
          <p className="col-span-2 text-label text-text-muted below-sm:col-span-1">
            Live graphs appear while the server is running.
          </p>
          <Storage folder={details?.folderBytes} world={details?.worldBytes} />
          <Tile
            label="Players"
            value={String(details?.playersPeakDay ?? 0)}
            unit="most at once in 24 hours"
          />
        </div>
      )}
    </Panel>
  );
}

function Storage({ folder, world }: { folder?: bigint; world?: bigint }) {
  return (
    <Tile
      label="Storage"
      value={folder !== undefined ? formatBytes(Number(folder)) : "—"}
      unit={folder !== undefined ? "in total" : undefined}
      note={
        world !== undefined
          ? `The world takes ${formatBytes(Number(world))}. Updated every few minutes.`
          : undefined
      }
    />
  );
}

function Tile({
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

/** Bytes per second between samples, from a cumulative counter. */
function rates(points: Point[], key: "diskRead" | "diskWrite"): number[] {
  const out: number[] = [];
  for (let i = 1; i < points.length; i++) {
    const [a, b] = [points[i - 1], points[i]];
    if (!a || !b) continue;
    const seconds = (b.t - a.t) / 1000;
    out.push(seconds > 0 ? Math.max(0, b[key] - a[key]) / seconds : 0);
  }
  return out;
}

/** The last value, averaged over a few samples to steady it. */
function last(values: number[]): number {
  const tail = values.slice(-5);
  return tail.length ? tail.reduce((a, b) => a + b, 0) / tail.length : 0;
}

function perSecond(bytes: number): string {
  if (bytes >= 1024 * 1024) return `${(bytes / 1024 / 1024).toFixed(1)} MB/s`;
  return `${Math.round(bytes / 1024)} KB/s`;
}

/** At most `n` evenly spaced points. */
function thin(points: Point[], n: number): Point[] {
  if (points.length <= n) return points;
  const step = points.length / n;
  return Array.from({ length: n }, (_, i) => points[Math.floor(i * step)]).filter((p): p is Point => !!p);
}
