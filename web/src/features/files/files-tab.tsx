/**
 * The Files tab: a folder browser beside an editor. Loaded on demand (see
 * files-panel.tsx). Everything here acts on one server's folder.
 */

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useBlocker } from "@tanstack/react-router";
import {
  Archive,
  ChevronRight,
  Copy,
  Download,
  File as FileIcon,
  FilePlus,
  Folder,
  FolderOpen,
  FolderPlus,
  HardDrive,
  Link2,
  MoreHorizontal,
  PackageOpen,
  Pencil,
  RefreshCw,
  Trash2,
  Upload,
  X,
} from "lucide-react";
import { type ReactNode, useRef, useState } from "react";
import { toast } from "sonner";

import { ConfirmDialog } from "@/components/confirm-dialog";
import { EmptyState } from "@/components/page";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import {
  ContextMenu,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuSeparator,
  ContextMenuTrigger,
} from "@/components/ui/context-menu";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { type FileEntry, FileKind } from "@/gen/nethernode/v1/files_pb";
import type { Server } from "@/gen/nethernode/v1/servers_pb";
import { errorMessage } from "@/lib/api";
import { trackJob } from "@/lib/jobs";
import { useNow } from "@/lib/use-now";
import { cn } from "@/lib/utils";

import { fileClient } from "./api";
import { downloadUrl, EditorPane } from "./editor-pane";
import { ExtractDialog, PromptDialog, TrashDialog } from "./file-dialogs";
import { ago, baseName, crumbs, formatSize, isZip, joinPath, parentPath, validName } from "./paths";
import { filesKey, listQuery } from "./queries";
import { UploadError, uploadFile } from "./upload";

type DialogState =
  | { kind: "mkdir" | "touch" }
  | { kind: "rename" | "copy" | "move" | "extract"; entry: FileEntry }
  | { kind: "compress" | "delete" | "move-many"; names: string[] }
  | { kind: "trash" };

interface Action {
  label: string;
  icon: ReactNode;
  run: () => void;
  destructive?: boolean;
  separated?: boolean;
}

export default function FilesTab({ server }: { server: Server }) {
  const client = useQueryClient();
  const now = useNow();
  const [dir, setDir] = useState("");
  const [openPath, setOpenPath] = useState<string | null>(null);
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [anchor, setAnchor] = useState<string | null>(null);
  const [dialog, setDialog] = useState<DialogState | null>(null);
  const [dirty, setDirty] = useState(false);
  const [pendingAction, setPendingAction] = useState<(() => void) | null>(null);
  const [dragging, setDragging] = useState(false);
  const uploadInput = useRef<HTMLInputElement>(null);
  const list = useQuery(listQuery(server.id, dir));
  const entries = list.data?.entries ?? [];

  // Leaving the page with unsaved edits asks first.
  const blocker = useBlocker({
    shouldBlockFn: () => dirty,
    withResolver: true,
    enableBeforeUnload: () => dirty,
  });

  /** Runs `action`, asking to discard unsaved edits first if there are any. */
  const guarded = (action: () => void) => {
    if (dirty) setPendingAction(() => action);
    else action();
  };

  const refresh = () => client.invalidateQueries({ queryKey: filesKey(server.id) });
  const call = useMutation({
    mutationFn: (work: () => Promise<unknown>) => work(),
    onSettled: refresh,
  });
  const act = async (work: () => Promise<unknown>, success?: string) => {
    try {
      await call.mutateAsync(work);
      if (success) toast.success(success);
    } catch (err) {
      toast.error(errorMessage(err));
    }
  };

  const goTo = (path: string) =>
    guarded(() => {
      setDir(path);
      setSelected(new Set());
      setAnchor(null);
      setOpenPath(null);
      setDirty(false);
    });

  const open = (entry: FileEntry) => {
    const path = joinPath(dir, entry.name);
    if (entry.kind === FileKind.DIRECTORY) goTo(path);
    else if (path !== openPath)
      guarded(() => {
        setOpenPath(path);
        setDirty(false);
      });
  };

  const toggle = (name: string, event: { shiftKey: boolean; metaKey: boolean; ctrlKey: boolean }) => {
    setSelected((prev) => {
      const next = new Set(prev);
      if (event.shiftKey && anchor) {
        const names = entries.map((e) => e.name);
        const from = Math.min(names.indexOf(anchor), names.indexOf(name));
        const to = Math.max(names.indexOf(anchor), names.indexOf(name));
        for (const n of names.slice(Math.max(0, from), to + 1)) next.add(n);
      } else if (next.has(name)) next.delete(name);
      else next.add(name);
      return next;
    });
    setAnchor(name);
  };

  const upload = async (files: FileList | File[]) => {
    for (const file of Array.from(files)) {
      const id = `upload:${dir}/${file.name}`;
      const send = async (overwrite: boolean) => {
        toast.loading(`Uploading ${file.name}…`, { id });
        try {
          await uploadFile(server.id, dir, file, overwrite, (f) =>
            toast.loading(`Uploading ${file.name}… ${Math.round(f * 100)}%`, { id }),
          );
          toast.success(`Uploaded ${file.name}.`, { id });
          void refresh();
        } catch (err) {
          if (err instanceof UploadError && err.exists) {
            toast.error(`${file.name} already exists here.`, {
              id,
              action: { label: "Replace", onClick: () => void send(true) },
              duration: 10_000,
            });
          } else {
            toast.error(err instanceof Error ? err.message : `Uploading ${file.name} failed.`, { id });
          }
        }
      };
      await send(false);
    }
  };

  const startJob = async (title: string, start: () => Promise<{ jobId: string }>) => {
    try {
      const { jobId } = await start();
      trackJob(jobId, title);
    } catch (err) {
      toast.error(errorMessage(err));
    }
  };

  const actionsFor = (entry: FileEntry): Action[] => {
    const path = joinPath(dir, entry.name);
    const isDir = entry.kind === FileKind.DIRECTORY;
    const actions: Action[] = [
      {
        label: isDir ? "Open" : "Open in editor",
        icon: isDir ? <FolderOpen /> : <FileIcon />,
        run: () => open(entry),
      },
    ];
    if (entry.kind === FileKind.FILE) {
      actions.push({
        label: "Download",
        icon: <Download />,
        run: () => window.location.assign(downloadUrl(server.id, path)),
      });
    }
    if (isZip(entry.name) && entry.kind === FileKind.FILE) {
      actions.push({
        label: "Extract…",
        icon: <PackageOpen />,
        run: () => setDialog({ kind: "extract", entry }),
      });
    }
    actions.push(
      {
        label: "Rename…",
        icon: <Pencil />,
        run: () => setDialog({ kind: "rename", entry }),
        separated: true,
      },
      { label: "Copy to…", icon: <Copy />, run: () => setDialog({ kind: "copy", entry }) },
      { label: "Move to…", icon: <FolderOpen />, run: () => setDialog({ kind: "move", entry }) },
      {
        label: "Compress…",
        icon: <Archive />,
        run: () => setDialog({ kind: "compress", names: [entry.name] }),
      },
      {
        label: "Delete",
        icon: <Trash2 />,
        run: () => setDialog({ kind: "delete", names: [entry.name] }),
        destructive: true,
        separated: true,
      },
    );
    return actions;
  };

  const selection = [...selected].filter((n) => entries.some((e) => e.name === n));
  const editorOpen = openPath !== null;

  return (
    <div
      className={cn(
        "grid gap-5 below-lg:grid-cols-1",
        editorOpen ? "grid-cols-[minmax(300px,0.9fr)_minmax(0,1.4fr)]" : "grid-cols-1",
      )}
    >
      <section
        aria-label="Server files"
        className={cn(
          "relative flex min-w-0 flex-col overflow-hidden rounded-[9px] border border-border bg-surface-subtle",
          editorOpen && "below-lg:hidden",
        )}
        onDragOver={(e) => {
          if (e.dataTransfer.types.includes("Files")) {
            e.preventDefault();
            setDragging(true);
          }
        }}
        onDragLeave={(e) => {
          if (!e.currentTarget.contains(e.relatedTarget as Node | null)) setDragging(false);
        }}
        onDrop={(e) => {
          e.preventDefault();
          setDragging(false);
          if (e.dataTransfer.files.length > 0) void upload(e.dataTransfer.files);
        }}
      >
        <div className="flex flex-wrap items-center gap-2 border-border border-b bg-surface px-[13px] py-[9px]">
          <nav aria-label="Folder" className="flex min-w-0 flex-1 items-center gap-1 text-label">
            <button
              type="button"
              onClick={() => goTo("")}
              className="flex shrink-0 items-center gap-1.5 rounded-sm px-1 py-0.5 text-text-secondary hover:text-text-primary"
              aria-current={dir === "" ? "location" : undefined}
            >
              <HardDrive className="size-[14px]" aria-hidden />
              {server.name}
            </button>
            {crumbs(dir).map((c) => (
              <span key={c.path} className="flex min-w-0 items-center gap-1">
                <ChevronRight className="size-3 shrink-0 text-text-muted" aria-hidden />
                <button
                  type="button"
                  onClick={() => goTo(c.path)}
                  className="truncate rounded-sm px-1 py-0.5 text-text-secondary hover:text-text-primary aria-[current]:text-text-primary"
                  aria-current={c.path === dir ? "location" : undefined}
                >
                  {c.name}
                </button>
              </span>
            ))}
          </nav>
          <div className="flex items-center gap-1.5">
            <DropdownMenu>
              <DropdownMenuTrigger asChild>
                <Button size="mini">
                  <FilePlus />
                  New
                </Button>
              </DropdownMenuTrigger>
              <DropdownMenuContent align="end">
                <DropdownMenuItem onSelect={() => setDialog({ kind: "mkdir" })}>
                  <FolderPlus />
                  Folder
                </DropdownMenuItem>
                <DropdownMenuItem onSelect={() => setDialog({ kind: "touch" })}>
                  <FilePlus />
                  Text file
                </DropdownMenuItem>
              </DropdownMenuContent>
            </DropdownMenu>
            <Button size="mini" onClick={() => uploadInput.current?.click()}>
              <Upload />
              Upload
            </Button>
            <input
              ref={uploadInput}
              type="file"
              multiple
              className="hidden"
              aria-label="Choose files to upload"
              onChange={(e) => {
                if (e.target.files) void upload(e.target.files);
                e.target.value = "";
              }}
            />
            <Button
              size="icon"
              variant="ghost"
              className="size-[27px]"
              aria-label="Trash"
              onClick={() => setDialog({ kind: "trash" })}
            >
              <Trash2 className="size-[14px]" />
            </Button>
            <Button
              size="icon"
              variant="ghost"
              className="size-[27px]"
              aria-label="Refresh"
              onClick={() => void refresh()}
            >
              <RefreshCw
                className={cn("size-[14px]", list.isFetching && "animate-spin motion-reduce:animate-none")}
              />
            </Button>
          </div>
        </div>

        {selection.length > 0 && (
          <div className="flex flex-wrap items-center gap-2 border-border border-b bg-nav-active px-[13px] py-2 text-meta text-nav-active-foreground">
            <span className="mr-auto">{selection.length} selected</span>
            <Button size="mini" onClick={() => setDialog({ kind: "compress", names: selection })}>
              <Archive />
              Compress
            </Button>
            <Button size="mini" onClick={() => setDialog({ kind: "move-many", names: selection })}>
              <FolderOpen />
              Move
            </Button>
            <Button
              size="mini"
              variant="danger"
              onClick={() => setDialog({ kind: "delete", names: selection })}
            >
              <Trash2 />
              Delete
            </Button>
            <Button
              size="icon"
              variant="ghost"
              className="size-[27px]"
              aria-label="Clear selection"
              onClick={() => setSelected(new Set())}
            >
              <X className="size-[14px]" />
            </Button>
          </div>
        )}

        <div className="min-h-[300px] flex-1">
          {list.isPending ? (
            <div className="m-3 h-40 animate-pulse rounded-md bg-surface" aria-busy />
          ) : list.isError ? (
            <EmptyState compact icon={<Folder />} title="Couldn't open this folder">
              {errorMessage(list.error)}
            </EmptyState>
          ) : (
            <ul aria-label={`Contents of ${dir || server.name}`} className="divide-y divide-divider">
              {dir && (
                <li>
                  <button
                    type="button"
                    onClick={() => goTo(parentPath(dir))}
                    className="flex w-full items-center gap-3 px-[13px] py-2 text-left text-label text-text-secondary hover:bg-nav-hover"
                  >
                    <span className="w-4" />
                    <Folder className="size-4 text-text-muted" aria-hidden />
                    ..
                    <span className="sr-only">(parent folder)</span>
                  </button>
                </li>
              )}
              {entries.length === 0 && (
                <li>
                  <EmptyState compact icon={<FolderOpen />} title="This folder is empty">
                    Drop files here or use Upload to add some.
                  </EmptyState>
                </li>
              )}
              {entries.map((entry) => (
                <EntryRow
                  key={entry.name}
                  entry={entry}
                  active={joinPath(dir, entry.name) === openPath}
                  selected={selected.has(entry.name)}
                  now={now}
                  actions={actionsFor(entry)}
                  onOpen={() => open(entry)}
                  onToggle={(e) => toggle(entry.name, e)}
                />
              ))}
            </ul>
          )}
        </div>

        {dragging && (
          <div className="pointer-events-none absolute inset-0 grid place-items-center rounded-[9px] border-2 border-primary border-dashed bg-overlay">
            <p className="flex items-center gap-2 font-semibold text-label">
              <Upload className="size-4 text-primary" aria-hidden />
              Drop to upload to {dir || server.name}
            </p>
          </div>
        )}
      </section>

      {openPath && (
        <EditorPane
          key={openPath}
          server={server}
          path={openPath}
          onDirtyChange={setDirty}
          onClose={() =>
            guarded(() => {
              setOpenPath(null);
              setDirty(false);
            })
          }
        />
      )}

      <FileDialogs
        server={server}
        dir={dir}
        dialog={dialog}
        onClose={() => setDialog(null)}
        act={act}
        startJob={startJob}
        onDeleted={(names) => {
          setSelected(new Set());
          if (
            openPath &&
            names.some((n) => openPath === joinPath(dir, n) || openPath.startsWith(`${joinPath(dir, n)}/`))
          ) {
            setOpenPath(null);
            setDirty(false);
          }
        }}
      />

      <ConfirmDialog
        open={pendingAction !== null || blocker.status === "blocked"}
        onOpenChange={(next) => {
          if (!next) {
            setPendingAction(null);
            if (blocker.status === "blocked") blocker.reset();
          }
        }}
        title="Discard unsaved changes?"
        description={`Your edits to ${openPath ? baseName(openPath) : "this file"} haven't been saved.`}
        confirmLabel="Discard changes"
        onConfirm={() => {
          setDirty(false);
          if (blocker.status === "blocked") blocker.proceed();
          pendingAction?.();
          setPendingAction(null);
        }}
      />
    </div>
  );
}

function EntryRow({
  entry,
  active,
  selected,
  now,
  actions,
  onOpen,
  onToggle,
}: {
  entry: FileEntry;
  active: boolean;
  selected: boolean;
  now: number;
  actions: Action[];
  onOpen: () => void;
  onToggle: (e: { shiftKey: boolean; metaKey: boolean; ctrlKey: boolean }) => void;
}) {
  const Icon =
    entry.kind === FileKind.DIRECTORY
      ? Folder
      : entry.kind === FileKind.SYMLINK
        ? Link2
        : isZip(entry.name)
          ? Archive
          : FileIcon;
  return (
    <ContextMenu>
      <ContextMenuTrigger asChild>
        <li
          className={cn(
            "group flex items-center gap-3 px-[13px] py-1.5 hover:bg-nav-hover",
            (selected || active) && "bg-nav-hover",
            active && "shadow-[inset_3px_0_var(--color-primary)]",
          )}
        >
          <Checkbox
            checked={selected}
            aria-label={`Select ${entry.name}`}
            onClick={(e) => {
              e.preventDefault();
              onToggle(e);
            }}
          />
          <button
            type="button"
            onClick={(e) => (e.metaKey || e.ctrlKey || e.shiftKey ? onToggle(e) : onOpen())}
            className="flex min-w-0 flex-1 items-center gap-2.5 py-1 text-left text-label"
          >
            <Icon
              className={cn(
                "size-4 shrink-0",
                entry.kind === FileKind.DIRECTORY ? "text-primary" : "text-text-muted",
              )}
              aria-hidden
            />
            <span className="truncate">{entry.name}</span>
            {entry.kind === FileKind.SYMLINK && <span className="sr-only">(link)</span>}
          </button>
          <span className="w-16 shrink-0 text-right text-meta text-text-muted tabular-nums below-sm:hidden">
            {entry.kind === FileKind.FILE ? formatSize(entry.size) : ""}
          </span>
          <span className="w-20 shrink-0 text-right text-meta text-text-muted below-md:hidden">
            {ago(entry.modifiedMs, now)}
          </span>
          <DropdownMenu>
            <DropdownMenuTrigger asChild>
              <Button size="icon" variant="ghost" className="size-7" aria-label={`Actions for ${entry.name}`}>
                <MoreHorizontal className="size-[15px]" />
              </Button>
            </DropdownMenuTrigger>
            <DropdownMenuContent align="end">
              {actions.map((a) => (
                <span key={a.label} className="contents">
                  {a.separated && <DropdownMenuSeparator />}
                  <DropdownMenuItem variant={a.destructive ? "destructive" : "default"} onSelect={a.run}>
                    {a.icon}
                    {a.label}
                  </DropdownMenuItem>
                </span>
              ))}
            </DropdownMenuContent>
          </DropdownMenu>
        </li>
      </ContextMenuTrigger>
      <ContextMenuContent>
        {actions.map((a) => (
          <span key={a.label} className="contents">
            {a.separated && <ContextMenuSeparator />}
            <ContextMenuItem variant={a.destructive ? "destructive" : "default"} onSelect={a.run}>
              {a.icon}
              {a.label}
            </ContextMenuItem>
          </span>
        ))}
      </ContextMenuContent>
    </ContextMenu>
  );
}

function FileDialogs({
  server,
  dir,
  dialog,
  onClose,
  act,
  startJob,
  onDeleted,
}: {
  server: Server;
  dir: string;
  dialog: DialogState | null;
  onClose: () => void;
  act: (work: () => Promise<unknown>, success?: string) => Promise<void>;
  startJob: (title: string, start: () => Promise<{ jobId: string }>) => Promise<void>;
  onDeleted: (names: string[]) => void;
}) {
  const serverId = server.id;
  const openChange = (open: boolean) => {
    if (!open) onClose();
  };
  // Submitting throws on failure so the dialog can show the error.
  const strict = async (work: () => Promise<unknown>, success: string) => {
    await work();
    toast.success(success);
  };
  const client = useQueryClient();
  const refresh = () => client.invalidateQueries({ queryKey: filesKey(serverId) });

  if (!dialog) return null;
  switch (dialog.kind) {
    case "mkdir":
    case "touch": {
      const folder = dialog.kind === "mkdir";
      return (
        <PromptDialog
          open
          onOpenChange={openChange}
          title={folder ? "New folder" : "New text file"}
          description={`In ${dir || server.name}.`}
          label="Name"
          initial={folder ? "New folder" : "notes.txt"}
          confirmLabel="Create"
          validate={validName}
          onSubmit={async (name) => {
            const path = joinPath(dir, name);
            await strict(
              () =>
                folder
                  ? fileClient.makeDirectory({ serverId, path })
                  : fileClient.writeFile({ serverId, path, content: new Uint8Array(), createOnly: true }),
              `Created ${name}.`,
            );
            void refresh();
          }}
        />
      );
    }
    case "rename":
      return (
        <PromptDialog
          open
          onOpenChange={openChange}
          title={`Rename ${dialog.entry.name}`}
          description="Renaming a world or plugin folder while the server runs can break it; stop the server first."
          label="New name"
          initial={dialog.entry.name}
          confirmLabel="Rename"
          validate={validName}
          onSubmit={async (name) => {
            await strict(
              () =>
                fileClient.renameFile({
                  serverId,
                  from: joinPath(dir, dialog.entry.name),
                  to: joinPath(dir, name),
                }),
              `Renamed to ${name}.`,
            );
            void refresh();
          }}
        />
      );
    case "copy":
    case "move": {
      const copy = dialog.kind === "copy";
      const from = joinPath(dir, dialog.entry.name);
      return (
        <PromptDialog
          open
          onOpenChange={openChange}
          title={`${copy ? "Copy" : "Move"} ${dialog.entry.name}`}
          description="Enter the new path, relative to the server folder. Existing files are never overwritten."
          label="Destination"
          initial={copy ? joinPath(dir, `${dialog.entry.name} copy`) : from}
          confirmLabel={copy ? "Copy" : "Move"}
          validate={(v) =>
            !v.trim() ? "Enter a path." : !copy && v.trim() === from ? "Choose a different place." : null
          }
          onSubmit={async (to) => {
            await strict(
              () =>
                copy
                  ? fileClient.copyFile({ serverId, from, to })
                  : fileClient.renameFile({ serverId, from, to }),
              `${copy ? "Copied" : "Moved"} ${dialog.entry.name}.`,
            );
            void refresh();
          }}
        />
      );
    }
    case "move-many":
      return (
        <PromptDialog
          open
          onOpenChange={openChange}
          title={`Move ${dialog.names.length} items`}
          description="Enter the destination folder, relative to the server folder. It must already exist."
          label="Destination folder"
          initial={dir}
          confirmLabel="Move"
          validate={(v) => (v.trim() === dir ? "Choose a different folder." : null)}
          onSubmit={async (target) => {
            await strict(
              async () => {
                for (const name of dialog.names) {
                  await fileClient.renameFile({
                    serverId,
                    from: joinPath(dir, name),
                    to: joinPath(target, name),
                  });
                }
              },
              `Moved ${dialog.names.length} items to ${target || server.name}.`,
            );
            onDeleted(dialog.names);
            void refresh();
          }}
        />
      );
    case "compress": {
      const single = dialog.names.length === 1 ? dialog.names[0] : undefined;
      return (
        <PromptDialog
          open
          onOpenChange={openChange}
          title={single ? `Compress ${single}` : `Compress ${dialog.names.length} items`}
          description="The archive is created in this folder. Compressing runs in the background; large worlds take a while."
          label="Archive name"
          initial={`${single ?? "archive"}.zip`}
          confirmLabel="Compress"
          validate={(v) =>
            validName(v) ?? (v.toLowerCase().endsWith(".zip") ? null : "The name must end in .zip.")
          }
          onSubmit={(name) =>
            startJob(`Compressing into ${name}…`, () =>
              fileClient.compressFiles({
                serverId,
                paths: dialog.names.map((n) => joinPath(dir, n)),
                destination: joinPath(dir, name),
              }),
            )
          }
        />
      );
    }
    case "extract": {
      const archive = joinPath(dir, dialog.entry.name);
      return (
        <ExtractDialog
          open
          onOpenChange={openChange}
          archive={archive}
          initial={dir}
          onSubmit={(destination, overwrite) =>
            startJob(`Extracting ${dialog.entry.name}…`, () =>
              fileClient.extractArchive({ serverId, archive, destination, overwrite }),
            )
          }
        />
      );
    }
    case "delete": {
      const n = dialog.names.length;
      return (
        <ConfirmDialog
          open
          onOpenChange={openChange}
          title={n === 1 ? `Delete ${dialog.names[0]}?` : `Delete ${n} items?`}
          description="Deleted items go to the trash, where you can restore them for 30 days."
          confirmLabel="Move to trash"
          onConfirm={() => {
            onClose();
            void act(
              () =>
                fileClient.deleteFiles({ serverId, paths: dialog.names.map((name) => joinPath(dir, name)) }),
              n === 1 ? `Moved ${dialog.names[0]} to the trash.` : `Moved ${n} items to the trash.`,
            ).then(() => onDeleted(dialog.names));
          }}
        />
      );
    }
    case "trash":
      return (
        <TrashDialog
          open
          onOpenChange={openChange}
          serverId={serverId}
          onRestore={async (id) => {
            const { path } = await fileClient.restoreFile({ serverId, trashId: id });
            toast.success(`Restored ${path}.`);
            void refresh();
          }}
          onPurge={async (id) => {
            await fileClient.purgeTrash({ serverId, trashId: id });
            void refresh();
          }}
        />
      );
  }
}
