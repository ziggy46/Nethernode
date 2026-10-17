import { useQuery } from "@tanstack/react-query";
import { Zap } from "lucide-react";
import { useState } from "react";

import { Panel } from "@/components/page";
import { type Server, ServerStatus } from "@/gen/nethernode/v1/servers_pb";

import { formatBytes } from "./format";
import { statsHistoryQuery } from "./queries";
import { useLiveStats, usePaused } from "./server-card";
import { type Point, Sparkline, useStatsSeries } from "./sparkline";
import { HistoryTiles, type Range, RangePicker, Tile } from "./stats-history";

/** Samples in the short charts (one a second). */
const RECENT = 300;
/** Points the hour-long players chart is thinned to. */
const HOUR_POINTS = 240;

/** The server's resources, tick health, players, and disk. */
export function PerformancePanel({ server }: { server: Server }) {
  const [range, setRange] = useState<Range>("hour");
  const running = server.status === ServerStatus.RUNNING;
  const series = useStatsSeries(server.id, running || server.status === ServerStatus.STARTING);
  const details = useQuery({ ...statsHistoryQuery(server.id), refetchInterval: 5 * 60_000 }).data;
  const live = useLiveStats(server.id);
  const paused = usePaused(server);
  const recent = series.slice(-RECENT);
  const graphs = running && recent.length > 1;

  const tick = [...recent].reverse().find((p) => p.mspt !== undefined);
  const players = live?.players ?? server.playersOnline;
  const writes = rates(recent, "diskWrite");
  const reads = rates(recent, "diskRead");

  return (
    <Panel>
      <div className="mb-[17px] flex flex-wrap items-center justify-between gap-3">
        <h2 className="flex items-center gap-2 font-semibold text-label tracking-[-0.3px]">
          <Zap className="size-[15px] text-text-muted" aria-hidden />
          Performance
        </h2>
        <RangePicker value={range} onChange={setRange} />
      </div>
      {range !== "hour" ? (
        <HistoryTiles serverId={server.id} range={range} playersMax={server.playersMax}>
          <Storage folder={details?.folderBytes} world={details?.worldBytes} />
        </HistoryTiles>
      ) : graphs ? (
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
            value={paused ? "Paused" : tick?.tps !== undefined ? tick.tps.toFixed(1) : "—"}
            unit={tick && !paused ? "TPS" : undefined}
            tone={
              paused || tick?.tps === undefined
                ? undefined
                : tick.tps >= 19.5
                  ? "good"
                  : tick.tps >= 15
                    ? "warn"
                    : "bad"
            }
            note={
              paused
                ? "Nobody's online, so Minecraft has paused the world. It resumes when someone joins (see “Pause when empty” in the game settings)."
                : tick?.mspt !== undefined
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
            Live graphs appear while the server is running. Pick 24 hours, 7 days or 30 days for its history.
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
