/** The pieces of the Configuration tab that help with changing versions. */

/** The newest release when the server is on an older one, else nothing. */
export function newerRelease(
  versions: readonly { id: string; snapshot: boolean }[],
  current: string,
): string | undefined {
  const releases = versions.filter((v) => !v.snapshot);
  const latest = releases[0]?.id;
  if (!latest || latest === current) return undefined;
  // A version that isn't listed (custom or very old): don't claim it's behind.
  return releases.findIndex((v) => v.id === current) > 0 ? latest : undefined;
}

/** Looks like a Minecraft release ("1.21.4", "26.3"), so checking mods for it is worth a request. */
export function isReleaseVersion(version: string): boolean {
  return /^\d+(\.\d+){1,2}$/.test(version);
}

/** Software that runs mods Modrinth can check. */
export const MOD_LOADERS = ["fabric", "quilt", "forge", "neoforge"];
