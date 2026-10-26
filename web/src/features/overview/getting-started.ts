import { type Listener, ListenerState } from "@/gen/nethernode/v1/proxy_pb";

export type ListenerStatus =
  | { kind: "listening"; port: number }
  | { kind: "failed"; port: number; error: string }
  | { kind: "off" };

/** Whether players can connect to the proxy: any listener bound, else the first that couldn't be. */
export function listenerStatus(
  listeners: readonly Pick<Listener, "state" | "port" | "error">[],
): ListenerStatus {
  const up = listeners.find((l) => l.state === ListenerState.LISTENING);
  if (up) return { kind: "listening", port: up.port };
  const failed = listeners.find((l) => l.state === ListenerState.FAILED);
  if (failed) return { kind: "failed", port: failed.port, error: failed.error };
  return { kind: "off" };
}

/** A name players would type to join: a server's own, else one under the first domain. */
export function joinAddress(
  servers: readonly { slug: string; hostnames: readonly string[] }[],
  domains: readonly string[],
): string | undefined {
  const routed = servers.find((s) => s.hostnames.length > 0);
  if (routed) return routed.hostnames[0];
  const [domain] = domains;
  if (!domain) return undefined;
  return `${servers[0]?.slug ?? "smp"}.${domain}`;
}
