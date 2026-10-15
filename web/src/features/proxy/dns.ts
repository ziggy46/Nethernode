import { type Listener, type Route, RouteKind } from "@/gen/nethernode/v1/proxy_pb";

export interface DnsRecord {
  type: "A" | "SRV";
  name: string;
  value: string;
}

/** The port Minecraft clients use when none is typed. */
export const MINECRAFT_PORT = 25565;

/**
 * The DNS records players need: a wildcard A (or AAAA) record per domain,
 * an A record per hostname outside those domains, and an SRV record for
 * exact hostnames when the proxy listens on a port other than 25565, so
 * players don't have to type it.
 */
export function dnsRecords(routes: Route[], listeners: Listener[], domains: string[] = []): DnsRecord[] {
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
  // One wildcard record per domain covers every name under it.
  const covered = (host: string) => domains.some((d) => host.endsWith(`.${d}`));
  for (const domain of domains) {
    add({ type: "A", name: `*.${domain}`, value: "This machine's public IP address" });
  }
  for (const route of named) {
    if (covered(route.hostname.replace(/^\*\./, "x."))) continue;
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
