import { useMutation, useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";

import { errorMessage } from "@/lib/api";
import { trackJob } from "@/lib/jobs";

import { backupClient, backupsKey } from "./api";

/** Starts a backup of one server, with a progress toast. */
export function useBackUpNow() {
  const client = useQueryClient();
  return useMutation({
    mutationFn: async (server: { id: string; name: string }) => {
      const { jobId } = await backupClient.createBackup({ serverId: server.id });
      trackJob(jobId, `Backing up ${server.name}…`);
    },
    onError: (error, server) => toast.error(`${server.name}: ${errorMessage(error)}`),
    onSettled: () => client.invalidateQueries({ queryKey: backupsKey }),
  });
}
