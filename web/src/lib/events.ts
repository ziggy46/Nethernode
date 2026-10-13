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

function invalidate(client: QueryClient, event: string) {
  if (event === "lagged") {
    // Some events were dropped; refetch everything visible.
    void client.invalidateQueries();
    return;
  }
  for (const queryKey of INVALIDATES[event] ?? []) {
    void client.invalidateQueries({ queryKey });
  }
}

/** Subscribes to server events for as long as the signed-in app is mounted. */
export function useServerEvents() {
  const client = useQueryClient();
  useEffect(() => {
    const source = new EventSource("/api/events");
    const handlers = Object.keys(INVALIDATES)
      .concat("lagged")
      .map((name) => {
        const handler = () => invalidate(client, name);
        source.addEventListener(name, handler);
        return [name, handler] as const;
      });
    // EventSource reconnects on its own; refetch after a gap so nothing is missed.
    let wasDown = false;
    source.onerror = () => {
      wasDown = true;
    };
    source.onopen = () => {
      if (wasDown) void client.invalidateQueries();
      wasDown = false;
    };
    return () => {
      for (const [name, handler] of handlers) source.removeEventListener(name, handler);
      source.close();
    };
  }, [client]);
}
