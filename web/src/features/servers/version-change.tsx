import { useMutation, useQuery } from "@tanstack/react-query";
import { PackageSearch, TriangleAlert } from "lucide-react";
import { useEffect, useId } from "react";

import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { ErrorAlert } from "@/components/ui/field";
import { modClient } from "@/features/catalog/api";
import { mcVersionsQuery } from "@/features/catalog/queries";
import { errorMessage } from "@/lib/api";
import { loaderName } from "./format";
import { isReleaseVersion, MOD_LOADERS, newerRelease } from "./versions";

/** Under the version box: a newer release exists, with a button to fill it in. */
export function VersionHint({
  loader,
  saved,
  value,
  onUse,
}: {
  loader: string;
  /** The version the server is on now. */
  saved: string;
  /** What's typed in the version box. */
  value: string;
  onUse: (version: string) => void;
}) {
  const versions = useQuery(mcVersionsQuery(loader, false));
  const newer = versions.data ? newerRelease(versions.data, saved) : undefined;
  if (!newer || newer === value.trim()) return null;
  return (
    <p className="-mt-2.5 mb-[17px] flex flex-wrap items-center gap-2 text-meta text-text-muted">
      {loaderName(loader)} {newer} is out.
      <Button type="button" size="mini" onClick={() => onUse(newer)}>
        Use {newer}
      </Button>
    </p>
  );
}

/** For a modded server heading to another version: which mods have no build for it yet. */
export function ModsReadiness({
  serverId,
  loader,
  saved,
  target,
}: {
  serverId: string;
  loader: string;
  saved: string;
  target: string;
}) {
  const check = useMutation({
    mutationFn: (mcVersion: string) => modClient.checkModUpdates({ serverId, mcVersion }),
  });
  const { reset } = check;
  // An answer is about one version; typing another starts over.
  // biome-ignore lint/correctness/useExhaustiveDependencies: `target` is the trigger
  useEffect(() => reset(), [target, reset]);

  if (!MOD_LOADERS.includes(loader) || target === saved || !isReleaseVersion(target)) return null;
  const answered = check.isSuccess && check.variables === target;
  const missing = answered ? check.data.mods.filter((m) => m.unsupported) : [];
  return (
    <div className="-mt-2.5 mb-[17px] text-meta text-text-muted" aria-live="polite">
      {answered ? (
        missing.length === 0 ? (
          <p>
            Every mod Modrinth recognizes ({check.data.mods.length}) has a build for {target}. Mods it doesn't
            know about aren't checked.
          </p>
        ) : (
          <div className="rounded-md border border-warning/30 bg-warning-subtle p-3 text-warning">
            <p className="flex items-center gap-1.5 font-[550]">
              <TriangleAlert className="size-3.5 shrink-0" aria-hidden />
              {missing.length === 1 ? "1 mod has" : `${missing.length} mods have`} no build for {target} yet
            </p>
            <p className="mt-1 text-text-secondary">
              {missing.map((m) => m.project?.title || m.filename).join(", ")}. They'd have to be removed or
              wait. Mods Modrinth doesn't know about aren't checked.
            </p>
          </div>
        )
      ) : (
        <Button type="button" size="mini" disabled={check.isPending} onClick={() => check.mutate(target)}>
          <PackageSearch />
          {check.isPending ? "Checking Modrinth…" : `Check mods for ${target}`}
        </Button>
      )}
      {check.isError && <p className="mt-1.5 text-danger">{errorMessage(check.error)}</p>}
    </div>
  );
}

/** Asks before a server moves to another Minecraft version or software, offering a backup first. */
export function VersionChangeDialog({
  open,
  onOpenChange,
  from,
  to,
  loaderChanged,
  backup,
  onBackupChange,
  pending,
  error,
  onConfirm,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  from: string;
  to: string;
  loaderChanged: boolean;
  backup: boolean;
  onBackupChange: (backup: boolean) => void;
  pending: boolean;
  error: string | undefined;
  onConfirm: () => void;
}) {
  const backupId = useId();
  return (
    <Dialog open={open} onOpenChange={(next) => !pending && onOpenChange(next)}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>Change to {to}?</DialogTitle>
          <DialogDescription>
            The server is on {from} now.{" "}
            {loaderChanged
              ? "Another kind of server can't use the old one's mods or plugins, and may not be able to read its world."
              : "A world is upgraded the first time a newer version loads it, and an older version can't open it again."}
          </DialogDescription>
        </DialogHeader>
        {error && <ErrorAlert>{error}</ErrorAlert>}
        <div className="mb-[17px] flex items-start gap-3">
          <Checkbox
            id={backupId}
            checked={backup}
            onCheckedChange={(v) => onBackupChange(v === true)}
            className="mt-0.5"
          />
          <label htmlFor={backupId} className="text-label">
            <span className="block font-[550]">Back it up first (recommended)</span>
            <span className="mt-0.5 block text-meta text-text-muted">
              {!backup
                ? "Without a backup there's no way back to the old world."
                : loaderChanged
                  ? `Restoring the backup brings the old world back; switch the software back to ${from.split(" ")[0]} yourself.`
                  : `Restoring the backup brings the old world back and puts the server back on ${from}.`}
            </span>
          </label>
        </div>
        <DialogFooter>
          <Button onClick={() => onOpenChange(false)} disabled={pending}>
            Cancel
          </Button>
          <Button variant="primary" onClick={onConfirm} disabled={pending}>
            {pending
              ? backup
                ? "Backing up…"
                : "Changing…"
              : backup
                ? "Back up and change"
                : "Change version"}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
