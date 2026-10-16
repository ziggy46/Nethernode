import { useQuery } from "@tanstack/react-query";
import { History, TriangleAlert } from "lucide-react";

import { EmptyState } from "@/components/page";
import { Button } from "@/components/ui/button";
import type { Server } from "@/gen/nethernode/v1/servers_pb";
import { errorMessage } from "@/lib/api";
import { useCanManage } from "@/lib/session";

import { bytes } from "../uploads/format";
import { backupsQuery } from "./api";
import { BackupTable } from "./backup-table";
import { SchedulesPanel } from "./schedules";
import { schedulesQuery } from "./schedules-api";
import { useBackUpNow } from "./use-back-up";

/** The Backups section of a server's page. */
export default function BackupsTab({ server }: { server: Server }) {
  const backups = useQuery(backupsQuery(server.id));
  const schedules = useQuery(schedulesQuery(server.id));
  const canManage = useCanManage();
  const backUp = useBackUpNow();

  const failed = backups.error ?? schedules.error;
  if (failed) {
    return (
      <div className="rounded-lg border border-divider border-dashed bg-surface-subtle">
        <EmptyState
          icon={<TriangleAlert />}
          title="Couldn't load backups"
          action={
            <Button
              onClick={() => {
                void backups.refetch();
                void schedules.refetch();
              }}
            >
              Try again
            </Button>
          }
        >
          {errorMessage(failed)}
        </EmptyState>
      </div>
    );
  }
  const list = backups.data;
  const timetable = schedules.data;
  if (!list || !timetable) {
    return (
      <div className="h-64 animate-pulse rounded-lg bg-surface-subtle" role="status" aria-busy>
        <span className="sr-only">Loading backups…</span>
      </div>
    );
  }

  const backUpButton = canManage && (
    <Button variant="primary" disabled={backUp.isPending} onClick={() => backUp.mutate(server)}>
      <History />
      Back up now
    </Button>
  );
  return (
    <>
      <SchedulesPanel
        schedules={timetable.schedules}
        servers={[{ id: server.id, name: server.name }]}
        serverId={server.id}
      />
      <div className="mb-[17px] flex items-center justify-between gap-3">
        <h2 className="font-semibold text-section">
          Backups
          <span className="ml-[7px] font-normal text-label text-text-muted tracking-normal">
            {list.backups.length}
          </span>
        </h2>
        {list.backups.length > 0 && (
          <div className="flex items-center gap-3">
            {list.freeBytes !== undefined && (
              <span className="text-meta text-text-muted below-sm:hidden">
                {bytes(Number(list.freeBytes))} free on{" "}
                {server.nodeId === "local" ? "this machine" : server.nodeName}
              </span>
            )}
            {backUpButton}
          </div>
        )}
      </div>
      <BackupTable backups={list.backups} showServer={false} emptyAction={backUpButton || undefined} />
    </>
  );
}
