import { queryOptions } from "@tanstack/react-query";

import { fileClient } from "./api";

export const filesKey = (serverId: string) => ["files", serverId] as const;

export const listQuery = (serverId: string, path: string) =>
  queryOptions({
    queryKey: [...filesKey(serverId), "list", path] as const,
    queryFn: () => fileClient.listFiles({ serverId, path }),
  });

export const trashQuery = (serverId: string) =>
  queryOptions({
    queryKey: [...filesKey(serverId), "trash"] as const,
    queryFn: () => fileClient.listTrash({ serverId }),
  });
