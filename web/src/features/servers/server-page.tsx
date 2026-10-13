import { useQuery } from "@tanstack/react-query";
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
  Zap,
} from "lucide-react";
import { Tabs } from "radix-ui";
import { useState } from "react";

import { ConfirmDialog } from "@/components/confirm-dialog";
import { EmptyState, Footer, Panel, PanelHeader } from "@/components/page";
import { ServerStatusBadge } from "@/components/status-badge";
import { Button } from "@/components/ui/button";
import { type Server, ServerStatus } from "@/gen/nethernode/v1/servers_pb";
import { errorMessage } from "@/lib/api";
import { healthQuery } from "@/lib/health";
import { useCanManage } from "@/lib/session";
import { useDocumentTitle } from "@/lib/use-document-title";
import { useNow } from "@/lib/use-now";
import { cn } from "@/lib/utils";

import { ConsolePanel } from "../console/console-panel";
import { FilesPanel } from "../files/files-panel";
import { PlayersPanel } from "../players/players-panel";
import { ConfigurationTab } from "./configuration-tab";
import { useDockerAvailable } from "./docker-notice";
import { formatBytes, gb, loaderName, memoryLimitBytes, versionLabel } from "./format";
import { type PullProgress, pullKey, serverQuery } from "./queries";
import { subtitle, useLiveStats } from "./server-card";
import { Sparkline, useStatsSeries } from "./sparkline";
import { StopDialog } from "./stop-dialog";
import { useServerAction } from "./use-server-actions";

export function ServerPage({ id }: { id: string }) {
  const server = useQuery(serverQuery(id));
  const health = useQuery(healthQuery);
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
          <p className="mb-1.5 font-[650] text-eyebrow text-primary uppercase">Server workspace</p>
          <h1 className="mb-[7px] truncate font-[650] text-page below-md:text-[25px]">{s.name}</h1>
          <p className="text-label text-text-muted">
            {loaderName(s.loader)} {versionLabel(s.mcVersion)}
            {address && (
              <>
                {" · "}
                <span className="font-mono">{address}</span>
              </>
            )}
          </p>
        </div>
        <ServerStatusBadge status={s.status} />
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
            ["files", "Files"],
            ["configuration", "Configuration"],
            ["backups", "Backups"],
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
          <PlayersPanel server={s} />
        </Tabs.Content>
        <Tabs.Content value="files" forceMount className="data-[state=inactive]:hidden">
          {filesVisited && <FilesPanel server={s} />}
        </Tabs.Content>
        <Tabs.Content value="configuration">
          <ConfigurationTab server={s} />
        </Tabs.Content>
        <Tabs.Content value="backups">
          <div className="rounded-lg border border-divider border-dashed bg-surface-subtle">
            <EmptyState icon={<History />} title="No backups yet">
              World snapshots and backup schedules arrive in a later update.
            </EmptyState>
          </div>
        </Tabs.Content>
      </Tabs.Root>
      <Footer version={health.data?.version} />
    </>
  );
}

function ConsoleTab({ server }: { server: Server }) {
  const running = server.status === ServerStatus.RUNNING;
  const series = useStatsSeries(server.id, running || server.status === ServerStatus.STARTING);
  const live = useLiveStats(server.id);
  const recent = series.slice(-300);

  return (
    <div className="grid grid-cols-[minmax(0,1fr)_260px] gap-5 below-lg:grid-cols-1">
      <div className="flex min-w-0 flex-col gap-5">
        {server.status === ServerStatus.INSTALLING ? (
          <section className="overflow-hidden rounded-[9px] border border-border bg-console">
            <div className="flex items-center gap-3 border-border border-b bg-surface px-[17px] py-[13px] text-meta">
              <SquareTerminal className="size-4" aria-hidden />
              Server console
              <span className="ml-auto">
                <ServerStatusBadge status={server.status} />
              </span>
            </div>
            <div className="flex min-h-[220px] flex-col items-center justify-center gap-3 p-8 text-center">
              <InstallProgress server={server} />
            </div>
          </section>
        ) : (
          <ConsolePanel server={server} />
        )}

        <Panel>
          <PanelHeader title="Performance" icon={<Zap />} />
          {running && recent.length > 1 ? (
            <div className="grid grid-cols-2 gap-6 below-sm:grid-cols-1">
              <div>
                <p className="text-meta text-text-muted">CPU · last 5 minutes</p>
                <p className="font-[650] text-title tabular-nums">{Math.round(live?.cpuPercent ?? 0)}%</p>
                <Sparkline values={recent.map((p) => p.cpu)} max={100} className="mt-2 h-16 w-full" />
              </div>
              <div>
                <p className="text-meta text-text-muted">Memory · last 5 minutes</p>
                <p className="font-[650] text-title tabular-nums">
                  {formatBytes(live?.memoryBytes ?? 0)}
                  <span className="ml-1 font-normal text-label text-text-muted">
                    / {formatBytes(live?.memoryLimitBytes ?? 0)}
                  </span>
                </p>
                <Sparkline
                  values={recent.map((p) => p.memory)}
                  max={recent[recent.length - 1]?.limit ?? 0}
                  className="mt-2 h-16 w-full"
                />
              </div>
            </div>
          ) : (
            <p className="text-label text-text-muted">Resource graphs appear while the server is running.</p>
          )}
        </Panel>
      </div>

      <aside className="flex flex-col gap-[18px] below-lg:grid below-lg:grid-cols-2 below-lg:gap-[15px] below-sm:grid-cols-1">
        <Controls server={server} />
        <Panel>
          <PanelHeader title="Protect your world" icon={<ShieldCheck />} />
          <p className="text-meta text-text-muted">
            Snapshots of this server's world and configuration arrive in a later update.
          </p>
          <Button className="mt-2.5 w-full" disabled>
            <History />
            Create backup
          </Button>
        </Panel>
      </aside>
    </div>
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
  const [stopOpen, setStopOpen] = useState(false);
  const [killOpen, setKillOpen] = useState(false);
  const up = server.status === ServerStatus.RUNNING || server.status === ServerStatus.STARTING;
  const busy = action.isPending || [ServerStatus.INSTALLING, ServerStatus.STOPPING].includes(server.status);
  const disabled = busy || !docker || !canManage;
  const limitBytes = memoryLimitBytes(server.settings?.memoryMb ?? 0);

  return (
    <Panel>
      <PanelHeader title="Server controls" icon={<Settings />} />
      {canManage && (
        <div className="mb-[18px] flex flex-wrap gap-2">
          {up ? (
            <Button variant="danger" className="flex-1" disabled={disabled} onClick={() => setStopOpen(true)}>
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
          <Button className="flex-1" disabled={disabled || !up} onClick={() => action.mutate("restart")}>
            <RotateCcw />
            Restart
          </Button>
        </div>
      )}
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
        open={stopOpen}
        onOpenChange={setStopOpen}
        name={server.name}
        players={server.playersOnline}
        onConfirm={() => action.mutate("stop")}
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

function Fact({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div className="flex justify-between gap-3 border-divider border-b py-[11px] last:border-b-0">
      <dt className="text-text-muted">{label}</dt>
      <dd className="text-right text-text-primary">{children}</dd>
    </div>
  );
}
