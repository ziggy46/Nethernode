import { useQuery } from "@tanstack/react-query";
import { CircleCheck, History, ShieldAlert, ShieldCheck, TriangleAlert } from "lucide-react";

import { EmptyState, Footer, PageHeader, Panel, SectionHeading } from "@/components/page";
import { Badge } from "@/components/status-badge";
import { Button } from "@/components/ui/button";
import { type Backup, BackupStatus } from "@/gen/nethernode/v1/backups_pb";
import { type Schedule, ScheduleAction } from "@/gen/nethernode/v1/schedules_pb";
import { errorMessage } from "@/lib/api";
import { healthQuery } from "@/lib/health";
import { useCanManage } from "@/lib/session";
import { ago } from "@/lib/time";
import { useDocumentTitle } from "@/lib/use-document-title";
import { useNow } from "@/lib/use-now";

import { serversQuery } from "../servers/queries";
import { bytes } from "../uploads/format";
import { backupsQuery } from "./api";
import { BackupTable } from "./backup-table";
import { SchedulesPanel } from "./schedules";
import { schedulesQuery } from "./schedules-api";
import { useBackUpNow } from "./use-back-up";

/** Every server's backups and schedules. */
export function BackupsPage() {
  useDocumentTitle("Backups");
  const health = useQuery(healthQuery);
  const servers = useQuery(serversQuery);
  const backups = useQuery(backupsQuery());
  const schedules = useQuery(schedulesQuery());
  const canManage = useCanManage();
  const backUp = useBackUpNow();
  const list = servers.data ?? [];

  const failed = [servers, backups, schedules].find((q) => q.isError);
  return (
    <>
      <PageHeader
        eyebrow="World protection"
        title="Keep every world safe."
        description="Automatic snapshots, so you can focus on the adventure."
        actions={
          canManage && (
            <Button
              variant="primary"
              disabled={list.length === 0 || backUp.isPending}
              onClick={() => {
                for (const s of list) backUp.mutate(s);
              }}
            >
              <History />
              Back up all
            </Button>
          )
        }
      />
      {failed ? (
        <div className="rounded-lg border border-divider border-dashed bg-surface-subtle">
          <EmptyState
            icon={<TriangleAlert />}
            title="Couldn't load backups"
            action={
              <Button
                onClick={() => {
                  void servers.refetch();
                  void backups.refetch();
                  void schedules.refetch();
                }}
              >
                Try again
              </Button>
            }
          >
            {errorMessage(failed.error)}
          </EmptyState>
        </div>
      ) : !servers.data || !backups.data || !schedules.data ? (
        <div className="h-64 animate-pulse rounded-lg bg-surface-subtle" role="status" aria-busy>
          <span className="sr-only">Loading backups…</span>
        </div>
      ) : (
        <>
          {list.length > 0 && (
            <Protection
              servers={list}
              schedules={schedules.data.schedules}
              backups={backups.data.backups}
              freeBytes={backups.data.freeBytes}
            />
          )}
          <SchedulesPanel schedules={schedules.data.schedules} servers={list} />
          <SectionHeading title="Snapshots" count={backups.data.backups.length} />
          <BackupTable backups={backups.data.backups} showServer />
        </>
      )}
      <Footer version={health.data?.version} />
    </>
  );
}

/** How well protected the servers are, in one line. */
function Protection({
  servers,
  schedules,
  backups,
  freeBytes,
}: {
  servers: { id: string }[];
  schedules: Schedule[];
  backups: Backup[];
  freeBytes: bigint | undefined;
}) {
  const now = useNow();
  const backupSchedules = schedules.filter((s) => s.enabled && s.action === ScheduleAction.BACKUP);
  const covered = servers.filter((s) => backupSchedules.some((b) => b.serverId === s.id)).length;
  const latest = backups.find((b) => b.status === BackupStatus.READY);
  const first = backupSchedules[0];
  const details = [
    first &&
      `${first.description || first.cron}${
        first.keepDaily || first.keepWeekly
          ? ` · keeps ${first.keepDaily} daily and ${first.keepWeekly} weekly`
          : " · keeps every backup"
      }`,
    latest ? `Last backup ${ago(latest.createdAtMs, now)}` : "No backups yet",
    freeBytes !== undefined && `${bytes(Number(freeBytes))} free`,
  ].filter(Boolean);

  const all = covered === servers.length;
  return (
    <Panel className="mb-6 flex items-center gap-3.5 below-sm:flex-wrap">
      <span className={all ? "text-success" : "text-text-muted"} aria-hidden>
        {all ? <ShieldCheck className="size-5" /> : <ShieldAlert className="size-5" />}
      </span>
      <div className="min-w-0 flex-1">
        <h2 className="font-semibold text-label">
          {all
            ? "Scheduled backups are on for every server"
            : covered > 0
              ? `Scheduled backups are on for ${covered} of ${servers.length} servers`
              : "No scheduled backups yet"}
        </h2>
        <p className="mt-1 text-meta text-text-muted">{details.join(" · ")}</p>
      </div>
      {all ? (
        <Badge tone="success" icon={<CircleCheck className="size-3" />}>
          Protected
        </Badge>
      ) : (
        <Badge tone="warning" icon={<ShieldAlert className="size-3" />}>
          {servers.length - covered} without a schedule
        </Badge>
      )}
    </Panel>
  );
}
