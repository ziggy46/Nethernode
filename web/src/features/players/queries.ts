import { queryOptions } from "@tanstack/react-query";

import { playerClient } from "./api";

export const playersKey = ["players"] as const;

export const playersQuery = (serverId?: string) =>
  queryOptions({
    queryKey: [...playersKey, serverId ?? "all"] as const,
    queryFn: () => playerClient.listPlayers({ serverId }),
  });

export const playerQuery = (uuid: string, serverId?: string) =>
  queryOptions({
    queryKey: [...playersKey, serverId ?? "all", "player", uuid] as const,
    queryFn: () => playerClient.getPlayer({ uuid, serverId }),
  });

export const playerDataQuery = (serverId: string, uuid: string) =>
  queryOptions({
    queryKey: [...playersKey, serverId, "data", uuid] as const,
    queryFn: () => playerClient.getPlayerData({ serverId, uuid }),
  });
