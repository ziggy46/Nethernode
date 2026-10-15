import { type QueryClient, useQueryClient } from "@tanstack/react-query";
import { useEffect } from "react";
import { toast } from "sonner";

import { type LiveStats, liveStatsKey, type PullProgress, pullKey } from "@/features/servers/queries";
import { uploadFinished } from "@/features/uploads/store";
import { jobFinished, jobProgress } from "@/lib/jobs";

/**
 * Bus event names sent by `/api/events`, mapped to the queries they make
 * stale. The UI never polls; it refetches when the server says something
 * changed.
 */
const INVALIDATES: Record<string, string[][]> = {
  player_joined: [["proxy", "connections"], ["players"]],
  player_left: [["proxy", "connections"], ["players"]],
  players_changed: [["players"], ["activity"]],
  proxy_connection_closed: [["proxy", "connections"]],
  node_changed: [["nodes"]],
  proxy_listener_changed: [["proxy", "listeners"]],
  server_status_changed: [["servers"], ["proxy", "routes"], ["players"], ["schedules"], ["activity"]],
  server_players: [["servers"]],
  server_operation_failed: [["servers"]],
};

/** Events whose payload updates the cache directly instead of refetching. */
type DataEvent =
  | {
      type: "server_stats";
      server_id: string;
      cpu_percent: number;
      memory_bytes: number;
      memory_limit_bytes: number;
    }
  | { type: "image_pull_progress"; server_id: string; percent: number | null; status: string }
  | { type: "server_operation_failed"; server_id: string; operation: string; message: string }
  | { type: "server_status_changed"; server_id: string; status: string }
  | {
      type: "upload_finished";
      server_id: string;
      upload_id: string;
      path: string;
      message: string;
      skipped: boolean;
      failed: boolean;
    }
  | { type: "file_job_progress"; server_id: string; job_id: string; kind: string; percent: number }
  | {
      type: "file_job_finished";
      server_id: string;
      job_id: string;
      kind: string;
      message: string;
      failed: boolean;
    };

const DATA_EVENTS = [
  "server_stats",
  "image_pull_progress",
  "server_operation_failed",
  "server_status_changed",
  "file_job_progress",
  "file_job_finished",
  "upload_finished",
];

function applyData(client: QueryClient, event: DataEvent) {
  switch (event.type) {
    case "server_stats": {
      const live: LiveStats = {
        cpuPercent: event.cpu_percent,
        memoryBytes: event.memory_bytes,
        memoryLimitBytes: event.memory_limit_bytes,
        atMs: Date.now(),
      };
      client.setQueryData(liveStatsKey(event.server_id), live);
      break;
    }
    case "image_pull_progress": {
      const pull: PullProgress = { percent: event.percent, status: event.status };
      client.setQueryData(pullKey(event.server_id), pull);
      break;
    }
    case "server_operation_failed": {
      // Crashes and failed operations stay visible until dismissed.
      const name = client
        .getQueryData<{ id: string; name: string }[]>(["servers"])
        ?.find((s) => s.id === event.server_id)?.name;
      toast.error(name ? `${name}: ${event.message}` : event.message, { duration: Number.POSITIVE_INFINITY });
      break;
    }
    case "file_job_progress":
      jobProgress(event.job_id, event.percent);
      break;
    case "file_job_finished":
      jobFinished(event.job_id, { message: event.message, failed: event.failed });
      void client.invalidateQueries({ queryKey: ["files", event.server_id] });
      void client.invalidateQueries({ queryKey: ["mods", event.server_id] });
      if (event.kind === "backup" || event.kind === "restore") {
        void client.invalidateQueries({ queryKey: ["backups"] });
        void client.invalidateQueries({ queryKey: ["schedules"] });
        void client.invalidateQueries({ queryKey: ["activity"] });
      }
      break;
    case "upload_finished":
      uploadFinished(event.upload_id, {
        path: event.path,
        message: event.message,
        skipped: event.skipped,
        failed: event.failed,
      });
      void client.invalidateQueries({ queryKey: ["files", event.server_id] });
      void client.invalidateQueries({ queryKey: ["mods", event.server_id] });
      void client.invalidateQueries({ queryKey: ["uploads"] });
      break;
    case "server_status_changed":
      if (event.status !== "starting" && event.status !== "running") {
        client.removeQueries({ queryKey: liveStatsKey(event.server_id) });
      }
      if (event.status !== "installing") client.removeQueries({ queryKey: pullKey(event.server_id) });
      break;
  }
}

/**
 * Busy servers emit an event for every server-list ping, so invalidations
 * are coalesced: each query refetches at most once per window.
 */
export const COALESCE_MS = 400;

/** Batches query invalidations into one flush per window. */
export function createInvalidator(client: QueryClient, windowMs = COALESCE_MS) {
  const pending = new Map<string, string[]>();
  let everything = false;
  let timer: ReturnType<typeof setTimeout> | undefined;

  const flush = () => {
    timer = undefined;
    if (everything) {
      void client.invalidateQueries();
    } else {
      for (const queryKey of pending.values()) void client.invalidateQueries({ queryKey });
    }
    pending.clear();
    everything = false;
  };

  return {
    /** Records that `event` happened. `lagged` means events were dropped. */
    note(event: string) {
      if (event === "lagged") everything = true;
      for (const key of INVALIDATES[event] ?? []) pending.set(JSON.stringify(key), key);
      if ((everything || pending.size > 0) && timer === undefined) timer = setTimeout(flush, windowMs);
    },
    /** Refetch everything after a reconnect gap. */
    all() {
      everything = true;
      if (timer === undefined) timer = setTimeout(flush, windowMs);
    },
    dispose() {
      if (timer !== undefined) clearTimeout(timer);
    },
  };
}

/** Subscribes to server events for as long as the signed-in app is mounted. */
export function useServerEvents() {
  const client = useQueryClient();
  useEffect(() => {
    const invalidator = createInvalidator(client);
    const source = new EventSource("/api/events");
    const names = [...new Set([...Object.keys(INVALIDATES), ...DATA_EVENTS, "lagged"])];
    const handlers = names.map((name) => {
      const handler = (message: MessageEvent<string>) => {
        if (DATA_EVENTS.includes(name)) {
          try {
            applyData(client, JSON.parse(message.data) as DataEvent);
          } catch {
            // A malformed event shouldn't break the stream.
          }
        }
        invalidator.note(name);
      };
      source.addEventListener(name, handler);
      return [name, handler] as const;
    });
    // EventSource reconnects on its own; refetch after a gap so nothing is missed.
    let wasDown = false;
    source.onerror = () => {
      wasDown = true;
    };
    source.onopen = () => {
      if (wasDown) invalidator.all();
      wasDown = false;
    };
    return () => {
      for (const [name, handler] of handlers) source.removeEventListener(name, handler);
      source.close();
      invalidator.dispose();
    };
  }, [client]);
}
