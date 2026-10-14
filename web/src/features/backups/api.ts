import { createClient } from "@connectrpc/connect";
import { queryOptions } from "@tanstack/react-query";

import { BackupService } from "@/gen/nethernode/v1/backups_pb";
import { transport } from "@/lib/api";

/** Backup RPCs. */
export const backupClient = createClient(BackupService, transport);

export const backupsKey = ["backups"] as const;

export const backupsQuery = (serverId?: string) =>
  queryOptions({
    queryKey: [...backupsKey, serverId ?? "all"] as const,
    queryFn: () => backupClient.listBackups({ serverId }),
  });
