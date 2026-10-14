import { createClient } from "@connectrpc/connect";
import { queryOptions } from "@tanstack/react-query";

import { ScheduleService } from "@/gen/nethernode/v1/schedules_pb";
import { transport } from "@/lib/api";

/** Schedule RPCs. Loaded with the backups views only. */
export const scheduleClient = createClient(ScheduleService, transport);

export const schedulesKey = ["schedules"] as const;

export const schedulesQuery = (serverId?: string) =>
  queryOptions({
    queryKey: [...schedulesKey, serverId ?? "all"] as const,
    queryFn: () => scheduleClient.listSchedules({ serverId }),
  });
