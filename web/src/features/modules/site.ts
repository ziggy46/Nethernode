/** Modules' own addresses, worked out as the server does it (`module_sites.rs`). */

interface RouteLike {
  hostname: string;
  serverId?: string | undefined;
}

/** The addresses players use for a server: exact routes, and `<slug>.<domain>` for slug wildcards. */
export function serverAddresses(server: { id: string; slug: string }, routes: RouteLike[]): string[] {
  const exact = routes
    .filter((r) => r.serverId === server.id && !r.hostname.includes("*"))
    .map((r) => r.hostname.toLowerCase());
  const bySlug = routes
    .filter((r) => !r.serverId && r.hostname.startsWith("*."))
    .map((r) => `${server.slug}.${r.hostname.slice(2).toLowerCase()}`);
  return [...new Set([...exact, ...bySlug])];
}

/** A module's hostnames: the prefix in front of each server address, or a whole hostname as it is. */
export function siteAddresses(webHost: string, addresses: string[]): string[] {
  const host = webHost.trim().toLowerCase().replace(/\.$/, "");
  if (!host) return [];
  return host.includes(".") ? [host] : addresses.map((a) => `${host}.${a}`);
}

/** Why an address won't do, if it won't. */
export function siteError(webHost: string): string | undefined {
  const host = webHost.trim().toLowerCase().replace(/\.$/, "");
  const label = /^[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?$/;
  if (!host) return "Enter a prefix like map, or a whole hostname.";
  if (host.length > 253 || !host.split(".").every((l) => label.test(l)))
    return "Use letters, digits and dashes, like map or map.example.com.";
  return undefined;
}
