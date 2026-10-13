const GIB = 1024 ** 3;
const MIB = 1024 ** 2;

/** "3.2 GB", "512 MB". Binary units, labeled the way players expect. */
export function formatBytes(bytes: number): string {
  if (bytes >= GIB) return `${(bytes / GIB).toFixed(bytes >= 10 * GIB ? 0 : 1)} GB`;
  return `${Math.round(bytes / MIB)} MB`;
}

/** Memory in GB with one decimal, for "used / limit" pairs. */
export function gb(bytes: number): string {
  return (bytes / GIB).toFixed(1);
}

/** "2h 34m", "5m", "3d 4h". */
export function formatUptime(ms: number): string {
  const minutes = Math.max(0, Math.floor(ms / 60_000));
  if (minutes < 1) return "under a minute";
  if (minutes < 60) return `${minutes}m`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours}h ${minutes % 60}m`;
  return `${Math.floor(hours / 24)}d ${hours % 24}h`;
}

const LOADER_NAMES: Record<string, string> = {
  vanilla: "Vanilla",
  paper: "Paper",
  purpur: "Purpur",
  fabric: "Fabric",
  forge: "Forge",
  neoforge: "NeoForge",
  quilt: "Quilt",
};

export const LOADERS = Object.keys(LOADER_NAMES);

export function loaderName(loader: string): string {
  return LOADER_NAMES[loader] ?? loader;
}

/** "Latest release" for LATEST, else the version as typed. */
export function versionLabel(version: string): string {
  if (version === "LATEST") return "Latest release";
  if (version === "SNAPSHOT") return "Latest snapshot";
  return version;
}

/**
 * Container memory limit for a Java heap of `heapMb`: the heap plus 25%, at
 * least 1 GiB more (NetherNode's defaults; Java needs room beyond the heap).
 */
export function memoryLimitBytes(heapMb: number): number {
  return (heapMb + Math.max(heapMb * 0.25, 1024)) * 1024 * 1024;
}
