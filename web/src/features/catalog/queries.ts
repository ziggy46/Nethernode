import { keepPreviousData, queryOptions } from "@tanstack/react-query";

import { Platform, type ProjectKind } from "@/gen/nethernode/v1/catalog_pb";

import { catalogClient } from "./api";

/** Version lists change slowly; the server caches them for six hours. */
const SLOW = 30 * 60 * 1000;

export const mcVersionsQuery = (loader: string, snapshots: boolean) =>
  queryOptions({
    queryKey: ["catalog", "mc", loader, snapshots] as const,
    queryFn: async () =>
      (await catalogClient.listMcVersions({ loader, includeSnapshots: snapshots })).versions,
    staleTime: SLOW,
  });

export const loaderVersionsQuery = (loader: string, mcVersion: string) =>
  queryOptions({
    queryKey: ["catalog", "builds", loader, mcVersion] as const,
    queryFn: async () => (await catalogClient.listLoaderVersions({ loader, mcVersion })).versions,
    enabled: loader !== "vanilla" && !!mcVersion,
    staleTime: SLOW,
  });

export const curseForgeStatusQuery = queryOptions({
  queryKey: ["catalog", "curseforge"] as const,
  queryFn: async () => (await catalogClient.getCurseForgeStatus({})).configured,
  staleTime: SLOW,
});

export interface SearchParams {
  platform: Platform;
  kind: ProjectKind;
  query: string;
  mcVersion?: string;
  loader?: string;
  offset?: number;
}

export const searchQuery = (p: SearchParams) =>
  queryOptions({
    queryKey: ["catalog", "search", p] as const,
    queryFn: () =>
      catalogClient.searchProjects({
        platform: p.platform,
        kind: p.kind,
        query: p.query,
        mcVersion: p.mcVersion ?? "",
        loader: p.loader ?? "",
        offset: p.offset ?? 0,
        limit: 20,
      }),
    placeholderData: keepPreviousData,
    staleTime: 5 * 60 * 1000,
  });

export const projectVersionsQuery = (platform: Platform, projectId: string, mcVersion = "", loader = "") =>
  queryOptions({
    queryKey: ["catalog", "versions", platform, projectId, mcVersion, loader] as const,
    queryFn: async () =>
      (await catalogClient.listProjectVersions({ platform, projectId, mcVersion, loader })).versions,
    enabled: !!projectId,
    staleTime: 5 * 60 * 1000,
  });

export const PLATFORM_NAMES: Record<Platform, string> = {
  [Platform.UNSPECIFIED]: "",
  [Platform.MODRINTH]: "Modrinth",
  [Platform.CURSEFORGE]: "CurseForge",
};

/** "1.2M", "34K". */
export function compactCount(n: number | bigint): string {
  return new Intl.NumberFormat(undefined, { notation: "compact", maximumFractionDigits: 1 }).format(
    Number(n),
  );
}
