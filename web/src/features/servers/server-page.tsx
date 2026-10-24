import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import {
  ArrowLeft,
  History,
  Loader,
  Play,
  RotateCcw,
  Settings,
  ShieldCheck,
  Square,
  SquareTerminal,
  TriangleAlert,
} from "lucide-react";
import { Tabs } from "radix-ui";
import { useState } from "react";

import { ConfirmDialog } from "@/components/confirm-dialog";
import { EmptyState, Panel, PanelHeader } from "@/components/page";
import { ServerStatusBadge } from "@/components/status-badge";
import { Button } from "@/components/ui/button";
import { CopyChip } from "@/components/ui/copy-chip";
import { BackupStatus } from "@/gen/nethernode/v1/backups_pb";
import { type Server, ServerStatus } from "@/gen/nethernode/v1/servers_pb";
import { errorMessage, serverClient } from "@/lib/api";
import { useCanManage } from "@/lib/session";
import { ago } from "@/lib/time";
import { withToast } from "@/lib/toast";
import { useDocumentTitle } from "@/lib/use-document-title";
import { useNow } from "@/lib/use-now";
import { cn } from "@/lib/utils";

import { backupsQuery } from "../backups/api";
import { BackupsPanel } from "../backups/backups-panel";
import { useBackUpNow } from "../backups/use-back-up";
import { ConsolePanel } from "../console/console-panel";
import { FilesPanel } from "../files/files-panel";
import { hasMods, ModsPanel } from "../mods/mods-panel";
import { ModulesPanel } from "../modules/modules-panel";
import { PlayersPanel } from "../players/players-panel";
import { BedrockJoin, BedrockPortChip, isBedrock } from "./bedrock";
import { BedrockPlayers } from "./bedrock-players";
import { ConfigurationTab } from "./configuration-tab";
import { CrashPanel } from "./crash-panel";
import { useDockerAvailable } from "./docker-notice";
import { gb, loaderName, memoryLimitBytes, versionLabel } from "./format";
import { PerformancePanel } from "./performance-panel";
import { type PullProgress, pullKey, serverQuery } from "./queries";
import { subtitle, useLiveStats } from "./server-card";
import { StopDialog } from "./stop-dialog";
import { useServerAction } from "./use-server-actions";

export function ServerPage({ id }: { id: string }) {
  const server = useQuery(serverQuery(id));
  const [tab, setTab] = useState("console");
  // The Files tab stays mounted once visited, so unsaved edits survive
  // switching tabs.
  const [filesVisited, setFilesVisited] = useState(false);
  useDocumentTitle(server.data?.name ?? "Server");

  if (server.isPending) {
    return <div className="h-64 animate-pulse rounded-lg bg-surface-subtle" aria-busy />;
  }
  if (server.isError) {
    return (
      <EmptyState
        icon={<TriangleAlert />}
        title="Couldn't load this server"
        action={
          <Button asChild>
            <Link to="/servers">All servers</Link>
          </Button>
        }
      >
        {errorMessage(server.error)}
      </EmptyState>
    );
  }
  const s = server.data;
  const address = s.hostnames[0];

  return (
    <>
      <Link
        to="/servers"
        className="mb-3.5 inline-flex items-center gap-1.5 text-meta text-text-muted hover:text-text-primary"
      >
        <ArrowLeft className="size-[13px]" aria-hidden />
        All servers
      </Link>
      <div className="mb-[25px] flex items-center justify-between gap-5 below-md:items-start">
        <div className="min-w-0">
          <p className="mb-1.5 font-[650] text-eyebrow text-primary uppercase">Server</p>
          <h1 className="mb-[7px] truncate font-[650] text-page below-md:text-[25px]">{s.name}</h1>
          <p className="flex flex-wrap items-center gap-x-2.5 gap-y-1.5 text-label text-text-muted">
            <span>
              {s.modpack
                ? `${s.modpack.title}${s.modpack.versionName ? ` ${s.modpack.versionName}` : ""} · Minecraft`
                : loaderName(s.loader)}{" "}
              {versionLabel(s.mcVersion)}
              {s.nodeId !== "local" && ` · on ${s.nodeName}`}
            </span>
            {isBedrock(s.loader) ? (
              <BedrockPortChip server={s} />
            ) : (
              address && <CopyChip text={address} mono className="text-label" />
            )}
          </p>
        </div>
      </div>

      <Tabs.Root
        value={tab}
        onValueChange={(value) => {
          setTab(value);
          if (value === "files") setFilesVisited(true);
        }}
      >
        <Tabs.List aria-label="Server sections" className="mb-[22px] flex gap-5 border-divider border-b">
          {[
            ["console", "Console"],
            ["players", "Players"],
            ...(hasMods(s) ? [["mods", "Mods"]] : []),
            ["files", "Files"],
            ["configuration", "Configuration"],
            ["backups", "Backups"],
            ["modules", "Modules"],
          ].map(([value, label]) => (
            <Tabs.Trigger
              key={value}
              value={value ?? ""}
              className="-mb-px border-transparent border-b-2 pt-2.5 pb-3 text-label text-text-muted transition-colors hover:text-text-primary data-[state=active]:border-primary data-[state=active]:text-primary"
            >
              {label}
            </Tabs.Trigger>
          ))}
        </Tabs.List>
        <Tabs.Content value="console">
          <ConsoleTab server={s} />
        </Tabs.Content>
        <Tabs.Content value="players">
          {isBedrock(s.loader) ? <BedrockPlayers server={s} /> : <PlayersPanel server={s} />}
        </Tabs.Content>
        {hasMods(s) && (
          <Tabs.Content value="mods">
            <ModsPanel server={s} />
          </Tabs.Content>
        )}
        <Tabs.Content value="files" forceMount className="data-[state=inactive]:hidden">
          {filesVisited && <FilesPanel server={s} />}
        </Tabs.Content>
        <Tabs.Content value="configuration">
          <ConfigurationTab server={s} />
        </Tabs.Content>
        <Tabs.Content value="backups">
          <BackupsPanel server={s} />
        </Tabs.Content>
        <Tabs.Content value="modules">
          <ModulesPanel server={s} />
        </Tabs.Content>
      </Tabs.Root>
    </>
  );
}

function ConsoleTab({ server }: { server: Server }) {
  const canManage = useCanManage();
  return (
    <div className="grid grid-cols-[minmax(0,1fr)_260px] gap-5 below-lg:grid-cols-1">
      <div className="flex min-w-0 flex-col gap-5">
        {server.status === ServerStatus.INSTALLING ? (
          <section className="overflow-hidden rounded-[9px] border border-border bg-console">
            <div className="flex items-center gap-3 border-border border-b bg-surface px-[17px] py-[13px] text-meta">
              <SquareTerminal className="size-4" aria-hidden />
              Server console
              <span className="ml-auto">
                <ServerStatusBadge
                  status={server.status}
                  nodeOffline={server.nodeOffline}
                  nodeUpdateNeeded={server.nodeUpdateNeeded}
                />
              </span>
            </div>
            <div className="flex min-h-[220px] flex-col items-center justify-center gap-3 p-8 text-center">
              <InstallProgress server={server} />
            </div>
          </section>
        ) : (
          <>
            {/* Mods failing to load can end with a clean exit, so stopped servers are checked too. */}
            {!isBedrock(server.loader) &&
              (server.status === ServerStatus.CRASHED || server.status === ServerStatus.STOPPED) && (
                <CrashPanel server={server} canManage={canManage} />
              )}
            <ConsolePanel server={server} />
          </>
        )}

        <PerformancePanel server={server} />
      </div>

      <aside className="flex flex-col gap-[18px] below-lg:grid below-lg:grid-cols-2 below-lg:gap-[15px] below-sm:grid-cols-1">
        <Controls server={server} />
        {isBedrock(server.loader) && <BedrockJoin server={server} />}
        <ProtectPanel server={server} />
      </aside>
    </div>
  );
}

function ProtectPanel({ server }: { server: Server }) {
  const canManage = useCanManage();
  const backups = useQuery(backupsQuery(server.id));
  const backUp = useBackUpNow();
  const now = useNow();
  const latest = backups.data?.backups.find((b) => b.status === BackupStatus.READY);
  const creating = backups.data?.backups.some((b) => b.status === BackupStatus.CREATING);
  return (
    <Panel>
      <PanelHeader title="Protect your world" icon={<ShieldCheck />} />
      <p className="text-meta text-text-muted">
        {backups.isError
          ? "Couldn't check this server's backups."
          : backups.isPending
            ? "Checking backups…"
            : creating
              ? "A backup is being made now."
              : latest
                ? `Last backup ${ago(latest.createdAtMs, now)}.`
                : "No backups yet. A backup captures the world, mods, and configuration."}
      </p>
      {canManage && (
        <Button
          className="mt-2.5 w-full"
          disabled={backUp.isPending || creating}
          onClick={() => backUp.mutate(server)}
        >
          <History />
          Create backup
        </Button>
      )}
    </Panel>
  );
}

function InstallProgress({ server }: { server: Server }) {
  const pull = useQuery<PullProgress | null>({
    queryKey: pullKey(server.id),
    queryFn: () => null,
    enabled: false,
  }).data;

  return (
    <>
      <Loader className="size-6 animate-spin text-primary motion-reduce:animate-none" aria-hidden />
      <p className="font-semibold">Installing {server.name}</p>
      <p className="max-w-[46ch] text-label text-text-muted" role="status">
        {pull
          ? `${pull.status}${pull.percent !== null ? ` · ${pull.percent}%` : ""}`
          : "Preparing the server…"}
      </p>
      {pull?.percent !== null && pull?.percent !== undefined && (
        <div className="h-1 w-64 overflow-hidden rounded-full bg-meter-track" aria-hidden>
          <div
            className="h-full bg-primary transition-[width] duration-500"
            style={{ width: `${pull.percent}%` }}
          />
        </div>
      )}
      <p className="max-w-[46ch] text-meta text-text-muted">
        The console appears once the server's container exists. The first start also downloads the server
        files
        {server.loader !== "vanilla" ? ` and sets up ${loaderName(server.loader)}` : ""}.
      </p>
    </>
  );
}

function Controls({ server }: { server: Server }) {
  const canManage = useCanManage();
  const docker = useDockerAvailable();
  const action = useServerAction(server.id, server.name);
  const live = useLiveStats(server.id);
  const now = useNow();
  const [stopOpen, setStopOpen] = useState<"stop" | "restart" | null>(null);
  const [killOpen, setKillOpen] = useState(false);
  const online = server.status === ServerStatus.RUNNING ? (server.playersOnline ?? 0) : 0;
  const up = server.status === ServerStatus.RUNNING || server.status === ServerStatus.STARTING;
  const busy = action.isPending || [ServerStatus.INSTALLING, ServerStatus.STOPPING].includes(server.status);
  // A server on another machine is reachable while that machine's agent is.
  const reachable = server.nodeId === "local" ? docker : !server.nodeOffline;
  const disabled = busy || !reachable || !canManage;
  const limitBytes = memoryLimitBytes(server.settings?.memoryMb ?? 0, server.loader);

  return (
    <Panel>
      <PanelHeader title="Server controls" icon={<Settings />} />
      {canManage && (
        <div className="mb-[18px] flex flex-wrap gap-2">
          {up ? (
            <Button
              variant="danger"
              className="flex-1"
              disabled={disabled}
              onClick={() => setStopOpen("stop")}
            >
              <Square />
              Stop
            </Button>
          ) : (
            <Button
              variant="primary"
              className="flex-1"
              disabled={disabled}
              onClick={() => action.mutate("start")}
            >
              <Play />
              {server.status === ServerStatus.SLEEPING ? "Wake" : "Start"}
            </Button>
          )}
          <Button
            className="flex-1"
            disabled={disabled || !up}
            // With people playing, ask first (and offer to warn them).
            onClick={() => (online > 0 ? setStopOpen("restart") : action.mutate("restart"))}
          >
            <RotateCcw />
            Restart
          </Button>
        </div>
      )}
      {server.countdown && <Countdown server={server} canManage={canManage} />}
      <dl className="text-meta">
        <Fact label="Players">
          {server.status === ServerStatus.RUNNING && server.playersOnline !== undefined
            ? `${server.playersOnline} / ${server.playersMax}`
            : "—"}
        </Fact>
        <Fact label="Memory">
          {live && up
            ? `${gb(live.memoryBytes)} / ${gb(live.memoryLimitBytes)} GB`
            : `— / ${gb(limitBytes)} GB`}
        </Fact>
        <Fact label="CPU usage">{live && up ? `${Math.round(live.cpuPercent)}%` : "—"}</Fact>
        <Fact label="Internal port">
          <span className="font-mono">{server.internalPort}</span>
        </Fact>
        <Fact label="Status">{subtitle(server, now)}</Fact>
      </dl>
      {server.restartRequired && (
        <p className="mt-3 text-meta text-warning">Settings changed. Restart to apply them.</p>
      )}
      {canManage && up && (
        <button
          type="button"
          disabled={disabled}
          onClick={() => setKillOpen(true)}
          className={cn(
            "mt-3 text-meta text-text-muted underline-offset-2 hover:text-danger hover:underline",
            disabled && "pointer-events-none opacity-50",
          )}
        >
          Force stop…
        </button>
      )}
      <StopDialog
        open={stopOpen !== null}
        onOpenChange={(open) => !open && setStopOpen(null)}
        name={server.name}
        players={online}
        restart={stopOpen === "restart"}
        onConfirm={(warnMinutes) =>
          action.mutate({ action: stopOpen === "restart" ? "restart" : "stop", warnMinutes })
        }
      />
      <ConfirmDialog
        open={killOpen}
        onOpenChange={setKillOpen}
        title="Force stop without saving?"
        description={`${server.name} stops immediately. Progress since the last autosave is lost.`}
        confirmLabel="Force stop"
        onConfirm={() => {
          action.mutate("kill");
          setKillOpen(false);
        }}
      />
    </Panel>
  );
}

/** A restart, stop or update counting down, with a way to call it off. */
function Countdown({ server, canManage }: { server: Server; canManage: boolean }) {
  const now = useNow(1000);
  const queryClient = useQueryClient();
  const cancel = useMutation({
    mutationFn: () => serverClient.cancelCountdown({ id: server.id }),
    onSuccess: () => void queryClient.invalidateQueries({ queryKey: ["servers"] }),
    onError: (e) => withToast((toast) => toast.error(errorMessage(e))),
  });
  const left = Math.max(0, Math.ceil((Number(server.countdownAtMs) - now) / 1000));
  const clock = `${Math.floor(left / 60)}:${String(left % 60).padStart(2, "0")}`;
  const what =
    server.countdown === "stop" ? "Stopping" : server.countdown === "update" ? "Updating" : "Restarting";
  return (
    <div
      className="mb-[18px] flex items-center justify-between gap-3 rounded-md border border-warning/30 bg-warning-subtle px-3 py-2.5"
      role="status"
    >
      <p className="text-label">
        <span className="font-[550]">
          {what} in <span className="font-mono tabular-nums">{clock}</span>
        </span>
        <span className="block text-meta text-text-muted">Players are being warned in chat.</span>
      </p>
      {canManage && (
        <Button size="mini" disabled={cancel.isPending} onClick={() => cancel.mutate()}>
          Call it off
        </Button>
      )}
    </div>
  );
}

function Fact({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div className="flex justify-between gap-3 border-divider border-b py-[11px] last:border-b-0">
      <dt className="text-text-muted">{label}</dt>
      <dd className="text-right text-text-primary">{children}</dd>
    </div>
  );
}
