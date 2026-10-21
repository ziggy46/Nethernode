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

/** The longest of `zones` that `name` is below (not the zone itself). */
function zoneOf(name: string, zones: string[]): string | undefined {
  return zones.filter((z) => name.endsWith(`.${z}`)).sort((a, b) => b.length - a.length)[0];
}

/**
 * A module's hostnames: the prefix in front of each server address, or a whole hostname as it is. Under a
 * domain Cloudflare manages (`flatZones`), the prefix joins with a dash so the name stays one level below the
 * domain, which is all Cloudflare's free certificate covers: map-mc1.example.com.
 */
export function siteAddresses(webHost: string, addresses: string[], flatZones: string[] = []): string[] {
  const host = webHost.trim().toLowerCase().replace(/\.$/, "");
  if (!host) return [];
  if (host.includes(".")) return [host];
  return addresses.map((a) => {
    const zone = zoneOf(a, flatZones);
    if (!zone) return `${host}.${a}`;
    const below = a.slice(0, a.length - zone.length - 1).replaceAll(".", "-");
    return `${host}-${below}.${zone}`;
  });
}

/** Whether a hostname is too deep for Cloudflare's free certificate (more than one level below its domain). */
export function beyondCertificate(name: string, zones: string[]): boolean {
  const zone = zoneOf(name, zones);
  return zone !== undefined && name.slice(0, name.length - zone.length - 1).includes(".");
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
