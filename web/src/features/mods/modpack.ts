import { Platform } from "@/gen/nethernode/v1/catalog_pb";

/** Compares Minecraft versions like 1.21.4 (numeric parts first). */
export function compareMinecraft(a: string, b: string): number {
  const parts = (v: string) => v.split(/[.\-+_]/).filter(Boolean);
  const [pa, pb] = [parts(a), parts(b)];
  for (let i = 0; i < Math.max(pa.length, pb.length); i++) {
    const [x, y] = [pa[i], pb[i]];
    if (x === undefined) return -1;
    if (y === undefined) return 1;
    const [nx, ny] = [Number(x), Number(y)];
    const diff =
      Number.isInteger(nx) && Number.isInteger(ny) ? nx - ny : x.localeCompare(y, "en", { numeric: true });
    if (diff !== 0) return Math.sign(diff);
  }
  return 0;
}

/** The newest Minecraft version a pack version is for. */
export function newestMinecraft(gameVersions: string[]): string | undefined {
  return [...gameVersions].sort(compareMinecraft).at(-1);
}

/**
 * The Minecraft version a pack version moves a server to, when that's newer
 * than what it runs (`LATEST` and the like aren't compared).
 */
export function minecraftUpgrade(current: string, gameVersions: string[]): string | undefined {
  const target = newestMinecraft(gameVersions);
  if (!target || !/^\d/.test(current)) return undefined;
  return compareMinecraft(target, current) > 0 ? target : undefined;
}

/** A pack version's page, for its changelog. */
export function versionPage(platform: Platform, pageUrl: string, versionId: string): string | undefined {
  if (!pageUrl) return undefined;
  const base = pageUrl.replace(/\/+$/, "");
  return platform === Platform.CURSEFORGE ? `${base}/files/${versionId}` : `${base}/version/${versionId}`;
}
