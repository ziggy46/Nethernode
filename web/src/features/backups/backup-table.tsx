import { useMutation, useQueryClient } from "@tanstack/react-query";
import {
  CircleAlert,
  Download,
  Ellipsis,
  History,
  Info,
  Loader,
  RotateCcw,
  ShieldCheck,
  Trash2,
} from "lucide-react";
import { useId, useState } from "react";
import { toast } from "sonner";

import { ConfirmDialog } from "@/components/confirm-dialog";
import { EmptyState } from "@/components/page";
import { Badge } from "@/components/status-badge";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { type Backup, BackupKind, BackupStatus } from "@/gen/nethernode/v1/backups_pb";
import { errorMessage } from "@/lib/api";
import { trackJob } from "@/lib/jobs";
import { useCanManage } from "@/lib/session";
import { ago, dateTime } from "@/lib/time";
import { useNow } from "@/lib/use-now";

import { useDockerAvailable } from "../servers/docker-notice";
import { bytes } from "../uploads/format";
import { backupClient, backupsKey } from "./api";

const KIND: Record<BackupKind, string> = {
  [BackupKind.UNSPECIFIED]: "Backup",
  [BackupKind.MANUAL]: "Manual",
  [BackupKind.SCHEDULED]: "Scheduled",
  [BackupKind.PRE_RESTORE]: "Before restore",
};

/** Backups, newest first, with download, restore, and delete. */
export function BackupTable({
  backups,
  showServer,
  emptyAction,
}: {
  backups: Backup[];
  showServer: boolean;
  emptyAction?: React.ReactNode;
}) {
  const now = useNow();
  const canManage = useCanManage();
  const [restoring, setRestoring] = useState<Backup | null>(null);
  const [deleting, setDeleting] = useState<Backup | null>(null);
  const [details, setDetails] = useState<Backup | null>(null);

  if (backups.length === 0) {
    return (
      <div className="rounded-lg border border-divider border-dashed bg-surface-subtle">
        <EmptyState icon={<History />} title="No backups yet" action={emptyAction}>
          Backups capture the whole server folder: worlds, mods, and configuration. Make one now, or add a
          schedule so it happens on its own.
        </EmptyState>
      </div>
    );
  }

  return (
    <>
      <div className="overflow-x-auto rounded-lg border border-border">
        <table className="w-full min-w-[560px] text-label">
          <thead className="bg-surface-subtle text-left text-meta text-text-muted">
            <tr>
              {showServer && <Th>Server</Th>}
              <Th>Created</Th>
              <Th>Size</Th>
              <Th>Type</Th>
              <th scope="col" className="border-divider border-b px-3 py-2.5">
                <span className="sr-only">Actions</span>
              </th>
            </tr>
          </thead>
          <tbody>
            {backups.map((b) => {
              const ready = b.status === BackupStatus.READY;
              return (
                <tr key={b.id} className="border-divider border-b last:border-b-0">
                  {showServer && (
                    <td className="px-3 py-2 font-semibold">
                      {b.serverName}
                      {!b.serverId && (
                        <span className="ml-1.5 font-normal text-meta text-text-muted">(deleted)</span>
                      )}
                    </td>
                  )}
                  <td className="whitespace-nowrap px-3 py-2 text-text-secondary">
                    <span title={dateTime(b.createdAtMs)}>{ago(b.createdAtMs, now)}</span>
                    {ready && b.message && (
                      <span className="block max-w-64 truncate text-meta text-text-muted" title={b.message}>
                        {b.message}
                      </span>
                    )}
                  </td>
                  <td className="whitespace-nowrap px-3 py-2 text-text-secondary tabular-nums">
                    {ready ? bytes(Number(b.sizeBytes)) : "—"}
                  </td>
                  <td className="px-3 py-2">
                    <StatusChip backup={b} />
                  </td>
                  <td className="px-3 py-2">
                    <div className="flex justify-end gap-1.5">
                      {canManage && ready && (
                        <Button size="mini" asChild>
                          <a href={`/api/backups/${b.id}/download`} download>
                            <Download />
                            Download
                          </a>
                        </Button>
                      )}
                      <DropdownMenu>
                        <DropdownMenuTrigger asChild>
                          <Button size="mini" variant="ghost" aria-label={`More actions for this backup`}>
                            <Ellipsis />
                          </Button>
                        </DropdownMenuTrigger>
                        <DropdownMenuContent align="end">
                          <DropdownMenuItem onSelect={() => setDetails(b)}>
                            <Info />
                            Details
                          </DropdownMenuItem>
                          {canManage && (
                            <>
                              <DropdownMenuItem
                                disabled={!ready || !b.serverId}
                                onSelect={() => setRestoring(b)}
                              >
                                <RotateCcw />
                                Restore…
                              </DropdownMenuItem>
                              <DropdownMenuSeparator />
                              <DropdownMenuItem
                                disabled={b.status === BackupStatus.CREATING}
                                onSelect={() => setDeleting(b)}
                                className="text-danger"
                              >
                                <Trash2 />
                                Delete…
                              </DropdownMenuItem>
                            </>
                          )}
                        </DropdownMenuContent>
                      </DropdownMenu>
                    </div>
                  </td>
                </tr>
              );
            })}
          </tbody>
        </table>
      </div>
      {restoring && <RestoreDialog backup={restoring} onClose={() => setRestoring(null)} />}
      {deleting && <DeleteDialog backup={deleting} onClose={() => setDeleting(null)} />}
      {details && <DetailsDialog backup={details} onClose={() => setDetails(null)} />}
    </>
  );
}

function Th({ children }: { children: React.ReactNode }) {
  return (
    <th scope="col" className="border-divider border-b px-3 py-2.5 font-medium">
      {children}
    </th>
  );
}

function StatusChip({ backup }: { backup: Backup }) {
  if (backup.status === BackupStatus.CREATING) {
    return (
      <Badge tone="warning" icon={<Loader className="size-3 animate-spin motion-reduce:animate-none" />}>
        Creating…
      </Badge>
    );
  }
  if (backup.status === BackupStatus.FAILED) {
    return (
      <Badge tone="danger" icon={<CircleAlert className="size-3" />}>
        Failed
      </Badge>
    );
  }
  return <Badge tone={backup.kind === BackupKind.PRE_RESTORE ? "info" : "muted"}>{KIND[backup.kind]}</Badge>;
}

function RestoreDialog({ backup, onClose }: { backup: Backup; onClose: () => void }) {
  const client = useQueryClient();
  const docker = useDockerAvailable();
  const [startAfter, setStartAfter] = useState(docker);
  const startId = useId();
  const restore = useMutation({
    mutationFn: () => backupClient.restoreBackup({ backupId: backup.id, startAfter: startAfter && docker }),
    onSuccess: ({ jobId }) => {
      trackJob(jobId, `Restoring ${backup.serverName}…`);
      void client.invalidateQueries({ queryKey: ["servers"] });
      onClose();
    },
  });
  return (
    <ConfirmDialog
      open
      onOpenChange={(open) => !open && onClose()}
      title={`Restore ${backup.serverName}?`}
      description={`The server goes back to how it was on ${dateTime(backup.createdAtMs)}. If it's running, it stops first and players are disconnected.`}
      confirmLabel="Restore"
      pending={restore.isPending}
      error={restore.error ? errorMessage(restore.error) : undefined}
      onConfirm={() => restore.mutate()}
    >
      <p className="mb-[17px] flex items-start gap-2 text-meta text-text-muted">
        <ShieldCheck className="mt-px size-3.5 shrink-0" aria-hidden />
        What's there now is backed up first, as a "Before restore" backup, so this can be undone.
      </p>
      <div className="mb-[17px] flex items-center gap-2.5">
        <Checkbox
          id={startId}
          checked={startAfter && docker}
          disabled={!docker}
          onCheckedChange={(v) => setStartAfter(v === true)}
        />
        <label htmlFor={startId} className="text-label">
          Start the server afterwards
        </label>
      </div>
    </ConfirmDialog>
  );
}

function DeleteDialog({ backup, onClose }: { backup: Backup; onClose: () => void }) {
  const client = useQueryClient();
  const remove = useMutation({
    mutationFn: () => backupClient.deleteBackup({ backupId: backup.id }),
    onSuccess: () => {
      toast.success("Backup deleted.");
      void client.invalidateQueries({ queryKey: backupsKey });
      onClose();
    },
  });
  return (
    <ConfirmDialog
      open
      onOpenChange={(open) => !open && onClose()}
      title="Delete this backup?"
      description={`The ${backup.serverName} backup from ${dateTime(backup.createdAtMs)} is removed from disk. This can't be undone.`}
      confirmLabel="Delete backup"
      pending={remove.isPending}
      error={remove.error ? errorMessage(remove.error) : undefined}
      onConfirm={() => remove.mutate()}
    />
  );
}

function DetailsDialog({ backup: b, onClose }: { backup: Backup; onClose: () => void }) {
  const raw = Number(b.rawBytes);
  const size = Number(b.sizeBytes);
  return (
    <Dialog open onOpenChange={(open) => !open && onClose()}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>{b.serverName} backup</DialogTitle>
          <DialogDescription>{dateTime(b.createdAtMs)}</DialogDescription>
        </DialogHeader>
        {b.message && (
          <p
            className={
              b.status === BackupStatus.FAILED ? "text-label text-danger" : "text-label text-text-secondary"
            }
          >
            {b.message}
          </p>
        )}
        <dl className="text-meta">
          <Row label="Type">{KIND[b.kind]}</Row>
          <Row label="Made by">{b.createdBy || "Schedule"}</Row>
          {b.status === BackupStatus.READY && (
            <>
              <Row label="Files">{Number(b.files).toLocaleString()}</Row>
              <Row label="Size">
                {bytes(size)}
                {raw > 0 && <span className="text-text-muted"> from {bytes(raw)}</span>}
              </Row>
            </>
          )}
          {b.finishedAtMs !== undefined && <Row label="Finished">{dateTime(b.finishedAtMs)}</Row>}
          <Row label="Contents">Server folder, without files the server downloads again</Row>
        </dl>
      </DialogContent>
    </Dialog>
  );
}

function Row({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div className="flex justify-between gap-3 border-divider border-b py-[9px] last:border-b-0">
      <dt className="text-text-muted">{label}</dt>
      <dd className="text-right text-text-primary">{children}</dd>
    </div>
  );
}
