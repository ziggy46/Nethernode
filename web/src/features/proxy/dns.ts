import { type Listener, type Route, RouteKind } from "@/gen/nethernode/v1/proxy_pb";

export interface DnsRecord {
  type: "A" | "SRV";
  name: string;
  value: string;
}

/** The port Minecraft clients use when none is typed. */
export const MINECRAFT_PORT = 25565;

/**
 * The DNS records players need for `routes`: an A (or AAAA) record per
 * hostname, and an SRV record for exact hostnames when the proxy listens on
 * a port other than 25565, so players don't have to type it.
 */
export function dnsRecords(routes: Route[], listeners: Listener[]): DnsRecord[] {
  const records: DnsRecord[] = [];
  const seen = new Set<string>();
  const add = (r: DnsRecord) => {
    const key = `${r.type} ${r.name} ${r.value}`;
    if (!seen.has(key)) {
      seen.add(key);
      records.push(r);
    }
  };
  const ports = [...new Set(listeners.filter((l) => l.enabled).map((l) => l.port))].filter(
    (p) => p !== MINECRAFT_PORT,
  );
  const named = routes.filter((r) => r.kind === RouteKind.EXACT || r.kind === RouteKind.WILDCARD);
  for (const route of named) {
    add({ type: "A", name: route.hostname, value: "This machine's public IP address" });
  }
  for (const route of named.filter((r) => r.kind === RouteKind.EXACT)) {
    for (const port of ports) {
      add({ type: "SRV", name: `_minecraft._tcp.${route.hostname}`, value: `0 5 ${port} ${route.hostname}` });
    }
  }
  return records;
}

/** Wildcard routes on a non-standard port: players must type the port. */
export function wildcardNeedsPort(routes: Route[], listeners: Listener[]): boolean {
  return (
    routes.some((r) => r.kind === RouteKind.WILDCARD) &&
    !listeners.some((l) => l.enabled && l.port === MINECRAFT_PORT)
  );
}
