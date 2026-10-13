import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect } from "react";

import type { StatSample } from "@/gen/nethernode/v1/servers_pb";
import { statsHistoryQuery } from "./queries";
import { useLiveStats } from "./server-card";

export interface Point {
  t: number;
  cpu: number;
  memory: number;
  limit: number;
}

const toPoint = (s: StatSample): Point => ({
  t: Number(s.atMs),
  cpu: s.cpuPercent,
  memory: Number(s.memoryBytes),
  limit: Number(s.memoryLimitBytes),
});

/**
 * Up to an hour of samples: the stored history, extended with live samples
 * as `server_stats` events arrive.
 */
export function useStatsSeries(id: string, enabled: boolean): Point[] {
  const client = useQueryClient();
  const history = useQuery({ ...statsHistoryQuery(id), enabled });
  const live = useLiveStats(id);

  useEffect(() => {
    if (!live || !enabled) return;
    client.setQueryData<StatSample[]>(statsHistoryQuery(id).queryKey, (prev) => {
      if (!prev) return prev;
      const last = prev[prev.length - 1];
      if (last && Number(last.atMs) >= live.atMs) return prev;
      const sample = {
        atMs: BigInt(live.atMs),
        cpuPercent: live.cpuPercent,
        memoryBytes: BigInt(live.memoryBytes),
        memoryLimitBytes: BigInt(live.memoryLimitBytes),
      } as StatSample;
      return [...prev.slice(-3599), sample];
    });
  }, [client, id, live, enabled]);

  return (history.data ?? []).map(toPoint);
}

/**
 * A minimal line chart (design system §3.6): 1.5 px primary line, a faint
 * area fill, and a lava dot on the latest value. Decorative: the caller
 * shows the numbers.
 */
export function Sparkline({ values, max, className }: { values: number[]; max: number; className?: string }) {
  const width = 300;
  const height = 64;
  if (values.length < 2) {
    return <div className={className} aria-hidden />;
  }
  const top = Math.max(max, ...values, 1);
  const step = width / (values.length - 1);
  const y = (v: number) => height - 4 - (Math.max(0, v) / top) * (height - 8);
  const line = values
    .map((v, i) => `${i === 0 ? "M" : "L"}${(i * step).toFixed(1)},${y(v).toFixed(1)}`)
    .join("");
  const last = values[values.length - 1] ?? 0;
  return (
    <svg viewBox={`0 0 ${width} ${height}`} preserveAspectRatio="none" className={className} aria-hidden>
      <defs>
        <linearGradient id="nn-spark-fill" x1="0" y1="0" x2="0" y2="1">
          <stop offset="0%" stopColor="var(--color-primary)" stopOpacity="0.1" />
          <stop offset="100%" stopColor="var(--color-primary)" stopOpacity="0" />
        </linearGradient>
      </defs>
      <path d={`${line}L${width},${height}L0,${height}Z`} fill="url(#nn-spark-fill)" />
      <path
        d={line}
        fill="none"
        stroke="var(--color-primary)"
        strokeWidth="1.5"
        vectorEffect="non-scaling-stroke"
      />
      <circle cx={width} cy={y(last)} r="2.5" fill="var(--color-lava)" />
    </svg>
  );
}
