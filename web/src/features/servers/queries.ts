import { queryOptions } from "@tanstack/react-query";

import { serverClient } from "@/lib/api";

export const serversQuery = queryOptions({
  queryKey: ["servers"],
  queryFn: async () => (await serverClient.listServers({})).servers,
});

export const serverQuery = (id: string) =>
  queryOptions({
    queryKey: ["servers", "one", id],
    queryFn: async () => {
      const { server } = await serverClient.getServer({ id });
      if (!server) throw new Error("server missing from response");
      return server;
    },
  });

export const statsHistoryQuery = (id: string) =>
  queryOptions({
    queryKey: ["servers", "stats", id],
    queryFn: async () => (await serverClient.getServerStats({ id })).samples,
    staleTime: Number.POSITIVE_INFINITY,
  });

export const hostQuery = queryOptions({
  queryKey: ["host"],
  queryFn: () => serverClient.getHostInfo({}),
  staleTime: 5 * 60_000,
});

/** Latest live sample per server, filled from `server_stats` events. */
export interface LiveStats {
  cpuPercent: number;
  memoryBytes: number;
  memoryLimitBytes: number;
  atMs: number;
}
export const liveStatsKey = (id: string) => ["servers", "live", id] as const;

/** Image download progress per server, from `image_pull_progress` events. */
export interface PullProgress {
  percent: number | null;
  status: string;
}
export const pullKey = (id: string) => ["servers", "pull", id] as const;
