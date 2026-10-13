import { useQuery } from "@tanstack/react-query";
import { RotateCcw, Trash2 } from "lucide-react";
import { useId, useState } from "react";

import { ConfirmDialog } from "@/components/confirm-dialog";
import { EmptyState } from "@/components/page";
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
import { ErrorAlert, Field } from "@/components/ui/field";
import { FileKind } from "@/gen/nethernode/v1/files_pb";
import { errorMessage } from "@/lib/api";
import { useNow } from "@/lib/use-now";

import { ago, baseName, formatSize } from "./paths";
import { trashQuery } from "./queries";

/** A one-field dialog: a name or a path, validated before submitting. */
export function PromptDialog({
  open,
  onOpenChange,
  title,
  description,
  label,
  initial,
  hint,
  confirmLabel,
  validate,
  onSubmit,
  children,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  title: string;
  description: string;
  label: string;
  initial: string;
  hint?: string;
  confirmLabel: string;
  validate?: (value: string) => string | null;
  /** Resolves when done; rejects with an error to show. */
  onSubmit: (value: string) => Promise<void>;
  children?: React.ReactNode;
}) {
  const [value, setValue] = useState(initial);
  const [error, setError] = useState<string | undefined>();
  const [pending, setPending] = useState(false);
  const invalid = validate?.(value) ?? null;

  return (
    <Dialog
      open={open}
      onOpenChange={(next) => {
        if (!next) {
          setError(undefined);
          setValue(initial);
        }
        onOpenChange(next);
      }}
    >
      <DialogContent>
        <DialogHeader>
          <DialogTitle>{title}</DialogTitle>
          <DialogDescription>{description}</DialogDescription>
        </DialogHeader>
        <form
          onSubmit={async (e) => {
            e.preventDefault();
            if (invalid) return;
            setPending(true);
            setError(undefined);
            try {
              await onSubmit(value.trim());
              onOpenChange(false);
            } catch (err) {
              setError(errorMessage(err));
            } finally {
              setPending(false);
            }
          }}
        >
          {error && <ErrorAlert>{error}</ErrorAlert>}
          <Field
            label={label}
            value={value}
            autoFocus
            onFocus={(e) => {
              // Select the name without its extension, like a desktop file manager.
              const dot = e.target.value.lastIndexOf(".");
              const slash = e.target.value.lastIndexOf("/");
              e.target.setSelectionRange(slash + 1, dot > slash + 1 ? dot : e.target.value.length);
            }}
            onChange={(e) => setValue(e.target.value)}
            error={value !== initial ? (invalid ?? undefined) : undefined}
            hint={hint}
            spellCheck={false}
            autoComplete="off"
          />
          {children}
          <DialogFooter>
            <Button onClick={() => onOpenChange(false)}>Cancel</Button>
            <Button type="submit" variant="primary" disabled={pending || !!invalid}>
              {pending ? "Working…" : confirmLabel}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}

/** Extract options: destination folder and whether to replace files. */
export function ExtractDialog({
  open,
  onOpenChange,
  archive,
  initial,
  onSubmit,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  archive: string;
  initial: string;
  onSubmit: (destination: string, overwrite: boolean) => Promise<void>;
}) {
  const [overwrite, setOverwrite] = useState(false);
  const overwriteId = useId();
  return (
    <PromptDialog
      open={open}
      onOpenChange={onOpenChange}
      title={`Extract ${baseName(archive)}`}
      description="Extraction runs in the background; you can keep working."
      label="Extract into folder"
      hint="Relative to the server folder. Created if it doesn't exist; leave empty for the server folder itself."
      initial={initial}
      confirmLabel="Extract"
      onSubmit={(destination) => onSubmit(destination, overwrite)}
    >
      <div className="mb-[17px] flex items-center gap-2.5">
        <Checkbox id={overwriteId} checked={overwrite} onCheckedChange={(v) => setOverwrite(v === true)} />
        <label htmlFor={overwriteId} className="text-label text-text-secondary">
          Replace files that already exist
        </label>
      </div>
    </PromptDialog>
  );
}

/** Deleted items, with restore and permanent deletion. */
export function TrashDialog({
  open,
  onOpenChange,
  serverId,
  onRestore,
  onPurge,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  serverId: string;
  onRestore: (id: string) => Promise<void>;
  onPurge: (id: string | undefined) => Promise<void>;
}) {
  const trash = useQuery({ ...trashQuery(serverId), enabled: open });
  const now = useNow();
  const [busy, setBusy] = useState<string | null>(null);
  const [emptyOpen, setEmptyOpen] = useState(false);
  const [error, setError] = useState<string | undefined>();
  const items = trash.data?.items ?? [];

  const run = async (id: string, work: () => Promise<void>) => {
    setBusy(id);
    setError(undefined);
    try {
      await work();
    } catch (err) {
      setError(errorMessage(err));
    } finally {
      setBusy(null);
    }
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="w-[600px]">
        <DialogHeader>
          <DialogTitle>Trash</DialogTitle>
          <DialogDescription>
            Deleted files stay here for {trash.data?.retentionDays ?? 30} days, then they're removed for good.
          </DialogDescription>
        </DialogHeader>
        {error && <ErrorAlert>{error}</ErrorAlert>}
        {trash.isPending ? (
          <div className="h-24 animate-pulse rounded-md bg-surface-subtle" aria-busy />
        ) : trash.isError ? (
          <ErrorAlert>{errorMessage(trash.error)}</ErrorAlert>
        ) : items.length === 0 ? (
          <EmptyState compact icon={<Trash2 />} title="The trash is empty">
            Deleted files and folders show up here.
          </EmptyState>
        ) : (
          <ul className="mb-4 max-h-[50vh] divide-y divide-divider overflow-auto rounded-md border border-divider">
            {items.map((item) => (
              <li key={item.id} className="flex items-center gap-3 px-3 py-2.5">
                <div className="min-w-0 flex-1">
                  <p className="truncate font-mono text-label">{item.originalPath}</p>
                  <p className="text-meta text-text-muted">
                    {item.kind === FileKind.DIRECTORY
                      ? "Folder"
                      : item.size !== undefined
                        ? formatSize(item.size)
                        : "File"}
                    {" · deleted "}
                    {ago(item.deletedMs, now)}
                  </p>
                </div>
                <Button
                  size="mini"
                  disabled={busy !== null}
                  onClick={() => run(item.id, () => onRestore(item.id))}
                >
                  <RotateCcw />
                  Restore
                </Button>
                <Button
                  size="mini"
                  variant="ghost"
                  aria-label={`Delete ${item.originalPath} forever`}
                  disabled={busy !== null}
                  onClick={() => run(item.id, () => onPurge(item.id))}
                >
                  <Trash2 />
                </Button>
              </li>
            ))}
          </ul>
        )}
        <DialogFooter>
          {items.length > 0 && (
            <Button
              variant="danger"
              className="mr-auto"
              disabled={busy !== null}
              onClick={() => setEmptyOpen(true)}
            >
              Empty trash
            </Button>
          )}
          <Button onClick={() => onOpenChange(false)}>Done</Button>
        </DialogFooter>
        <ConfirmDialog
          open={emptyOpen}
          onOpenChange={setEmptyOpen}
          title="Empty the trash?"
          description={`${items.length} item${items.length === 1 ? "" : "s"} will be deleted for good. This can't be undone.`}
          confirmLabel="Empty trash"
          pending={busy === "all"}
          onConfirm={() =>
            run("all", async () => {
              await onPurge(undefined);
              setEmptyOpen(false);
            })
          }
        />
      </DialogContent>
    </Dialog>
  );
}
