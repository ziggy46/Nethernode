import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  ArrowUpCircle,
  CircleCheck,
  CircleDashed,
  ExternalLink,
  PackagePlus,
  Puzzle,
  RefreshCw,
  Trash2,
  TriangleAlert,
  Upload,
} from "lucide-react";
import { useRef, useState } from "react";
import { toast } from "sonner";

import { ConfirmDialog } from "@/components/confirm-dialog";
import { EmptyState } from "@/components/page";
import { Badge } from "@/components/status-badge";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Switch } from "@/components/ui/switch";
import { Platform, ProjectKind } from "@/gen/nethernode/v1/catalog_pb";
import type { InstalledMod, ModUpdate } from "@/gen/nethernode/v1/mods_pb";
import { type Server, ServerStatus } from "@/gen/nethernode/v1/servers_pb";
import { errorMessage, serverClient } from "@/lib/api";
import { trackJob } from "@/lib/jobs";
import { cn } from "@/lib/utils";

import { modClient } from "../catalog/api";
import { PlatformTabs, ProjectIcon, ProjectSearch } from "../catalog/project-search";
import { curseForgeStatusQuery } from "../catalog/queries";
import { loaderName } from "../servers/format";
import { DropOverlay, useFileDrop } from "../uploads/drop";
import { bytes } from "../uploads/format";
import { enqueue } from "../uploads/store";

const modsKey = (serverId: string) => ["mods", serverId] as const;

/** The Mods section of a modded server's page. */
export default function ModsTab({ server }: { server: Server }) {
  const client = useQueryClient();
  const list = useQuery({
    queryKey: modsKey(server.id),
    queryFn: () => modClient.listMods({ serverId: server.id }),
  });
  const [updates, setUpdates] = useState<Map<string, ModUpdate> | null>(null);
  const [adding, setAdding] = useState(false);
  const [removing, setRemoving] = useState<InstalledMod | null>(null);
  const picker = useRef<HTMLInputElement>(null);
  const refresh = () => client.invalidateQueries({ queryKey: modsKey(server.id) });
  const running = server.status === ServerStatus.RUNNING;

  const upload = (files: File[], dir = "mods") => {
    const jars = files.filter((f) => f.name.endsWith(".jar") || f.name.endsWith(".zip"));
    if (jars.length < files.length)
      toast.warning("Only .jar files go in the mods folder; the rest were skipped.");
    if (jars.length > 0)
      enqueue({ serverId: server.id, serverName: server.name, dir }, jars, {
        conflict: "overwrite",
        extract: false,
      });
  };
  const drop = useFileDrop((files) => upload(files));

  const check = useMutation({
    mutationFn: () => modClient.checkModUpdates({ serverId: server.id }),
    onSuccess: (r) => {
      setUpdates(new Map(r.mods.map((m) => [m.filename, m])));
      const n = r.mods.filter((m) => m.latest).length;
      toast.success(
        n ? `${n} update${n === 1 ? "" : "s"} available.` : "Everything Modrinth knows about is up to date.",
      );
    },
    onError: (e) => toast.error(errorMessage(e)),
  });
  const act = useMutation({
    mutationFn: async (work: () => Promise<unknown>) => work(),
    onSettled: refresh,
    onError: (e) => toast.error(errorMessage(e)),
  });

  if (list.isPending) return <div className="h-64 animate-pulse rounded-lg bg-surface-subtle" aria-busy />;
  if (list.isError) {
    return (
      <EmptyState icon={<TriangleAlert />} title="Couldn't read the mods folder">
        {errorMessage(list.error)}
      </EmptyState>
    );
  }
  const data = list.data;
  if (!data.supported) {
    return (
      <div className="rounded-lg border border-divider border-dashed bg-surface-subtle">
        <EmptyState icon={<Puzzle />} title="This server doesn't run mods">
          Mods need Fabric, Quilt, Forge, or NeoForge. Plugins for Paper go in the plugins folder (Files tab).
        </EmptyState>
      </div>
    );
  }
  const loader = data.loader;
  const enabled = data.mods.filter((m) => m.enabled).length;

  return (
    <div {...drop.handlers} className="relative">
      {drop.dragging && <DropOverlay label="Drop .jar files to add them to mods/" />}
      <input
        ref={picker}
        type="file"
        multiple
        accept=".jar"
        hidden
        aria-label="Choose mod files"
        onChange={(e) => {
          upload(Array.from(e.target.files ?? []));
          e.target.value = "";
        }}
      />
      {data.manualDownloads.length > 0 && (
        <ManualDownloads
          server={server}
          data={data}
          onUpload={(files) => upload(files, data.downloadsFolder)}
        />
      )}
      <div className="mb-4 flex flex-wrap items-center gap-2">
        <p className="mr-auto text-label text-text-secondary">
          {enabled} of {data.mods.length} mods on
          {loader ? ` · ${loaderName(loader)}` : ""} · Minecraft {data.mcVersion}
        </p>
        <Button
          size="mini"
          onClick={() => check.mutate()}
          disabled={check.isPending || data.mods.length === 0}
        >
          <RefreshCw className={cn(check.isPending && "animate-spin")} />
          Check for updates
        </Button>
        <Button size="mini" onClick={() => picker.current?.click()}>
          <Upload />
          Upload
        </Button>
        <Button size="mini" variant="primary" onClick={() => setAdding(true)}>
          <PackagePlus />
          Add mods
        </Button>
      </div>
      {running && (
        <p className="mb-3 text-meta text-text-muted">Changes take effect the next time the server starts.</p>
      )}
      {data.mods.length === 0 ? (
        <div className="rounded-lg border border-divider border-dashed bg-surface-subtle">
          <EmptyState icon={<Puzzle />} title="No mods yet">
            Add mods from Modrinth or CurseForge, or drop .jar files here.
          </EmptyState>
        </div>
      ) : (
        <ul className="divide-y divide-divider rounded-lg border border-border">
          {data.mods.map((mod) => {
            const update = updates?.get(mod.filename);
            return (
              <li
                key={mod.filename}
                className={cn("flex items-center gap-3 px-3.5 py-2.5", !mod.enabled && "opacity-60")}
              >
                {mod.hasIcon ? (
                  <ProjectIcon
                    size={36}
                    url={`/api/servers/${server.id}/mods/icon?file=${encodeURIComponent(mod.filename)}`}
                  />
                ) : (
                  <ProjectIcon size={36} url={update?.project?.iconUrl ?? ""} />
                )}
                <div className="min-w-0 flex-1">
                  <p className="flex flex-wrap items-center gap-x-2 gap-y-1">
                    <span className="truncate font-medium text-label" title={mod.description || undefined}>
                      {mod.name || update?.project?.title || mod.filename}
                    </span>
                    {mod.version && <span className="text-meta text-text-muted">{mod.version}</span>}
                    {mod.environment === "client" && (
                      <Badge tone="warning" icon={<TriangleAlert className="size-3" />}>
                        Client only
                      </Badge>
                    )}
                    {update?.latest && (
                      <Badge tone="info" icon={<ArrowUpCircle className="size-3" />}>
                        {update.latest.versionNumber} available
                      </Badge>
                    )}
                  </p>
                  <p className="truncate font-mono text-meta text-text-muted" title={mod.filename}>
                    {mod.filename} · {bytes(Number(mod.size))}
                  </p>
                </div>
                {update?.latest && (
                  <Button
                    size="mini"
                    disabled={act.isPending}
                    onClick={() =>
                      act.mutate(async () => {
                        const { jobId } = await modClient.updateMod({
                          serverId: server.id,
                          filename: mod.filename,
                          versionId: update.latest?.id ?? "",
                        });
                        trackJob(jobId, `Updating ${mod.name || mod.filename}…`);
                        setUpdates((u) => {
                          const next = new Map(u);
                          next.delete(mod.filename);
                          return next;
                        });
                      })
                    }
                  >
                    Update
                  </Button>
                )}
                <Switch
                  checked={mod.enabled}
                  aria-label={`${mod.enabled ? "Turn off" : "Turn on"} ${mod.name || mod.filename}`}
                  disabled={act.isPending}
                  onCheckedChange={(on) =>
                    act.mutate(() =>
                      modClient.setModEnabled({ serverId: server.id, filename: mod.filename, enabled: on }),
                    )
                  }
                />
                <Button
                  size="icon"
                  variant="ghost"
                  className="size-8"
                  aria-label={`Remove ${mod.name || mod.filename}`}
                  onClick={() => setRemoving(mod)}
                >
                  <Trash2 />
                </Button>
              </li>
            );
          })}
        </ul>
      )}
      <ConfirmDialog
        open={removing !== null}
        onOpenChange={(o) => !o && setRemoving(null)}
        title={`Remove ${removing?.name || removing?.filename}?`}
        description="It moves to the trash (Files tab), where you can restore it for 30 days."
        confirmLabel="Remove"
        onConfirm={() => {
          const mod = removing;
          setRemoving(null);
          if (mod) act.mutate(() => modClient.deleteMod({ serverId: server.id, filename: mod.filename }));
        }}
      />
      {adding && (
        <AddMods
          server={server}
          mcVersion={data.mcVersion}
          loader={loader}
          installed={new Set(updates ? [...updates.values()].map((u) => u.project?.id ?? "") : [])}
          onClose={() => setAdding(false)}
        />
      )}
    </div>
  );
}

function AddMods({
  server,
  mcVersion,
  loader,
  installed,
  onClose,
}: {
  server: Server;
  mcVersion: string;
  loader: string;
  installed: Set<string>;
  onClose: () => void;
}) {
  const [platform, setPlatform] = useState(Platform.MODRINTH);
  const curseForgeKey = useQuery(curseForgeStatusQuery);
  const [started, setStarted] = useState<Set<string>>(new Set());
  const install = useMutation({
    mutationFn: async ({ id, title }: { id: string; title: string }) => {
      const { jobId } = await modClient.installMod({ serverId: server.id, platform, projectId: id });
      trackJob(jobId, `Installing ${title}…`);
      setStarted((s) => new Set(s).add(id));
    },
    onError: (e) => toast.error(errorMessage(e)),
  });
  return (
    <Dialog open onOpenChange={(o) => !o && onClose()}>
      <DialogContent className="w-[640px]">
        <DialogHeader>
          <DialogTitle>Add mods</DialogTitle>
          <DialogDescription>
            Showing mods for Minecraft {mcVersion}
            {loader ? ` with ${loaderName(loader)}` : ""} that run on servers. Mods they require come along
            (Modrinth).
          </DialogDescription>
        </DialogHeader>
        <div className="mb-3">
          <PlatformTabs value={platform} onChange={setPlatform} />
        </div>
        {platform === Platform.CURSEFORGE && curseForgeKey.data === false ? (
          <EmptyState compact icon={<TriangleAlert />} title="CurseForge needs an API key">
            Add one in Settings to search and install from CurseForge.
          </EmptyState>
        ) : (
          <div className="max-h-[55vh] overflow-y-auto pr-1">
            <ProjectSearch
              platform={platform}
              kind={ProjectKind.MOD}
              mcVersion={mcVersion}
              loader={loader || undefined}
              action={(p) =>
                installed.has(p.id) || started.has(p.id) ? (
                  <Badge tone="success" icon={<CircleCheck className="size-3" />}>
                    {started.has(p.id) ? "Installing" : "Installed"}
                  </Badge>
                ) : (
                  <Button
                    size="mini"
                    disabled={install.isPending}
                    onClick={() => install.mutate({ id: p.id, title: p.title })}
                  >
                    Install
                  </Button>
                )
              }
            />
          </div>
        )}
      </DialogContent>
    </Dialog>
  );
}

/** CurseForge files the pack needs that have to be downloaded by hand. */
function ManualDownloads({
  server,
  data,
  onUpload,
}: {
  server: Server;
  data: {
    manualDownloads: { mod: string; filename: string; downloadPage: string; present: boolean }[];
    downloadsFolder: string;
  };
  onUpload: (files: File[]) => void;
}) {
  const picker = useRef<HTMLInputElement>(null);
  const missing = data.manualDownloads.filter((f) => !f.present).length;
  const start = useMutation({
    mutationFn: () => serverClient.startServer({ id: server.id }),
    onSuccess: () => toast.success("Starting; the pack picks up the files you uploaded."),
    onError: (e) => toast.error(errorMessage(e)),
  });
  return (
    <section
      aria-label="Files to download by hand"
      className="mb-5 rounded-lg border border-warning/30 bg-warning-subtle px-4 py-3.5"
    >
      <h3 className="mb-1 flex items-center gap-2 font-semibold text-label text-warning">
        <TriangleAlert className="size-4" aria-hidden />
        {missing > 0
          ? `${missing} file${missing === 1 ? "" : "s"} must be downloaded by hand`
          : "All files are uploaded"}
      </h3>
      <p className="mb-3 text-meta text-text-secondary">
        {missing > 0
          ? "Their authors don't allow other apps to download them. Download each one from CurseForge, then drop them here (or upload them into "
          : "Start the server again to finish installing the pack ("}
        <span className="font-mono">{data.downloadsFolder}</span>
        {missing > 0 ? ")." : ")."}
      </p>
      <ul className="mb-3 flex flex-col gap-1.5">
        {data.manualDownloads.map((f) => (
          <li key={f.filename} className="flex items-center gap-2 text-meta">
            {f.present ? (
              <CircleCheck className="size-4 shrink-0 text-success" aria-label="Uploaded" />
            ) : (
              <CircleDashed className="size-4 shrink-0 text-text-muted" aria-label="Not uploaded yet" />
            )}
            <span className="min-w-0 flex-1 truncate">
              {f.mod} <span className="font-mono text-text-muted">{f.filename}</span>
            </span>
            <a
              href={f.downloadPage}
              target="_blank"
              rel="noreferrer noopener"
              className="inline-flex shrink-0 items-center gap-1 text-text-primary underline underline-offset-2"
            >
              Download page
              <ExternalLink className="size-3" aria-hidden />
            </a>
          </li>
        ))}
      </ul>
      <input
        ref={picker}
        type="file"
        multiple
        hidden
        aria-label="Choose the downloaded files"
        onChange={(e) => {
          onUpload(Array.from(e.target.files ?? []));
          e.target.value = "";
        }}
      />
      <div className="flex gap-2">
        <Button size="mini" onClick={() => picker.current?.click()}>
          <Upload />
          Upload downloaded files
        </Button>
        {missing === 0 && (
          <Button size="mini" variant="primary" disabled={start.isPending} onClick={() => start.mutate()}>
            Start server
          </Button>
        )}
      </div>
    </section>
  );
}
