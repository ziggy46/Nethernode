import { type QueryClient, useQueryClient } from "@tanstack/react-query";
import { useEffect } from "react";

/**
 * Bus event names sent by `/api/events`, mapped to the queries they make
 * stale. The UI never polls; it refetches when the server says something
 * changed.
 */
const INVALIDATES: Record<string, string[][]> = {
  player_joined: [["proxy", "connections"]],
  player_left: [["proxy", "connections"]],
  proxy_connection_closed: [["proxy", "connections"]],
  proxy_listener_changed: [["proxy", "listeners"]],
};

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
    const names = [...Object.keys(INVALIDATES), "lagged"];
    const handlers = names.map((name) => {
      const handler = () => invalidator.note(name);
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
