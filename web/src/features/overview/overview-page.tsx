import { useQueries, useQuery } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import {
  ArrowRight,
  Box,
  Cpu,
  History,
  MemoryStick,
  Plus,
  Server,
  ShieldCheck,
  TriangleAlert,
  Users,
} from "lucide-react";
import { lazy, Suspense, useState } from "react";

import {
  EmptyState,
  Footer,
  MetricCard,
  PageHeader,
  Panel,
  PanelHeader,
  SectionHeading,
} from "@/components/page";
import { ResourceMeter } from "@/components/resource-meter";
import { Button } from "@/components/ui/button";
import { backupsQuery } from "@/features/backups/api";
import { DockerNotice, useDockerAvailable } from "@/features/servers/docker-notice";
import { gb } from "@/features/servers/format";
import { hostQuery, type LiveStats, liveStatsKey, serversQuery } from "@/features/servers/queries";
import { ServerGrid } from "@/features/servers/server-grid";
import { bytes } from "@/features/uploads/format";
import { BackupStatus } from "@/gen/nethernode/v1/backups_pb";
import { ServerStatus, type Server as ServerT } from "@/gen/nethernode/v1/servers_pb";
import { errorMessage } from "@/lib/api";
import { healthQuery } from "@/lib/health";
import { useCanManage } from "@/lib/session";
import { ago } from "@/lib/time";
import { useDocumentTitle } from "@/lib/use-document-title";
import { useNow } from "@/lib/use-now";

const ActivityFeed = lazy(() => import("@/features/audit/activity-feed"));

/** The newest finished backup, from any server. */
function LatestBackup() {
  const backups = useQuery(backupsQuery());
  const now = useNow();
  const latest = backups.data?.backups.find((b) => b.status === BackupStatus.READY);
  return (
    <MetricCard
      label="Latest backup"
      icon={<ShieldCheck />}
      value={latest ? ago(latest.createdAtMs, now) : backups.data ? "None" : "—"}
      note={
        backups.isError
          ? "Couldn't check backups"
          : !backups.data
            ? "Checking backups…"
            : latest
              ? `${latest.serverName} · ${bytes(Number(latest.sizeBytes))}`
              : "Back up from the Backups page"
      }
    />
  );
}

/** Live samples for every running server, from the event stream. */
function useLiveTotals(servers: ServerT[]) {
  const running = servers.filter((s) => s.status === ServerStatus.RUNNING);
  const samples = useQueries({
    queries: running.map((s) => ({
      queryKey: liveStatsKey(s.id),
      queryFn: (): LiveStats | null => null,
      enabled: false,
    })),
  });
  let memory = 0;
  let cpu = 0;
  let measured = 0;
  for (const q of samples) {
    if (q.data) {
      memory += q.data.memoryBytes;
      cpu += q.data.cpuPercent;
      measured += 1;
    }
  }
  return { memory, cpu, measured, running: running.length };
}

const CreateServerDialog = lazy(() =>
  import("@/features/servers/create-server-dialog").then((m) => ({ default: m.CreateServerDialog })),
);

export function OverviewPage() {
  useDocumentTitle("Overview");
  const health = useQuery(healthQuery);
  const servers = useQuery(serversQuery);
  const host = useQuery(hostQuery);
  const canManage = useCanManage();
  const docker = useDockerAvailable();
  const [creating, setCreating] = useState(false);

  const list = servers.data ?? [];
  const totals = useLiveTotals(list);
  const players = list.reduce(
    (n, s) => n + (s.status === ServerStatus.RUNNING ? (s.playersOnline ?? 0) : 0),
    0,
  );
  const hostMemory = host.data?.dockerAvailable ? Number(host.data.memoryBytes) : 0;
  const offline = list.length - totals.running;

  return (
    <>
      <PageHeader
        eyebrow="Your control center"
        title="Your worlds, under control."
        description="A good day to build something. Here's how your servers are doing."
        actions={
          <>
            <Button asChild variant="subtle" className="below-md:hidden">
              <Link to="/backups">
                <History />
                Backups
              </Link>
            </Button>
            {canManage && (
              <Button variant="primary" disabled={!docker} onClick={() => setCreating(true)}>
                <Plus />
                Create server
              </Button>
            )}
          </>
        }
      />
      <DockerNotice />

      <div className="mb-[30px] grid grid-cols-4 gap-3.5 below-xl:gap-2.5 below-md:mb-6 below-md:grid-cols-2">
        <MetricCard
          label="Active servers"
          icon={<Server />}
          value={servers.isSuccess ? totals.running : "—"}
          unit={servers.isSuccess ? `/ ${list.length} server${list.length === 1 ? "" : "s"}` : undefined}
          note={
            list.length === 0 ? (
              "No servers yet"
            ) : (
              <>
                <span className="text-success">● {totals.running} running</span>
                {" · "}
                {offline} offline
              </>
            )
          }
        />
        <MetricCard
          label="Players online"
          icon={<Users />}
          value={servers.isSuccess ? players : "—"}
          unit="across your worlds"
          note={
            players === 0
              ? "No one online right now"
              : `In ${list.filter((s) => (s.playersOnline ?? 0) > 0).length} world(s)`
          }
        />
        <MetricCard
          label="Memory usage"
          icon={<MemoryStick />}
          value={
            totals.running > 0 && totals.measured > 0 ? gb(totals.memory) : totals.running === 0 ? "0" : "—"
          }
          unit={hostMemory ? `/ ${gb(hostMemory)} GB` : undefined}
          note={
            hostMemory
              ? `${Math.round((totals.memory / hostMemory) * 100)}% of host memory, used by servers`
              : "Host memory unknown"
          }
        />
        <LatestBackup />
      </div>

      <div className="grid grid-cols-[minmax(0,1fr)_var(--nn-right-rail-width)] gap-6 below-xl:gap-[18px] below-lg:grid-cols-1 below-md:gap-[22px]">
        <section aria-labelledby="servers-heading">
          <div id="servers-heading">
            <SectionHeading
              title="Your servers"
              count={list.length}
              caption="One workspace. Endless worlds."
            />
          </div>
          {servers.isPending ? (
            <div
              className="h-[300px] animate-pulse rounded-lg border border-divider bg-surface-subtle"
              aria-busy
            />
          ) : servers.isError ? (
            <Panel>
              <EmptyState compact icon={<TriangleAlert />} title="Couldn't load servers">
                {errorMessage(servers.error)}
              </EmptyState>
            </Panel>
          ) : list.length === 0 ? (
            <div className="rounded-lg border border-divider border-dashed bg-surface-subtle">
              <EmptyState
                icon={<Server />}
                title="No servers yet"
                action={
                  canManage && (
                    <Button variant="primary" disabled={!docker} onClick={() => setCreating(true)}>
                      <Plus />
                      Create your first server
                    </Button>
                  )
                }
              >
                Pick a server type and version; NetherNode downloads and sets it up for you.
              </EmptyState>
            </div>
          ) : (
            <ServerGrid servers={list} />
          )}
        </section>

        <aside
          aria-label="Workspace details"
          className="flex flex-col gap-[18px] below-lg:grid below-lg:grid-cols-2 below-lg:gap-[15px] below-sm:grid-cols-1"
        >
          <HostPanel totals={totals} />
          <Panel>
            <PanelHeader title="Recent activity" icon={<History />} />
            <Suspense fallback={<div className="h-24 animate-pulse rounded-md bg-surface" aria-busy />}>
              <ActivityFeed />
            </Suspense>
          </Panel>
          <section className="rounded-lg border border-primary/35 border-dashed bg-primary-subtle p-4 below-lg:col-span-full below-lg:flex below-lg:items-center below-lg:gap-5 below-sm:block">
            <h3 className="mb-1.5 flex items-center gap-[7px] font-[550] text-label">
              <Box className="size-[15px] text-primary" aria-hidden />
              Your next adventure
            </h3>
            <p className="mb-3 text-meta text-text-muted below-lg:m-0 below-lg:flex-1 below-sm:my-2">
              Explore modpacks and spin up a new world in a few clicks.
            </p>
            <Button asChild variant="accent" size="mini" className="w-full below-lg:w-auto below-sm:w-full">
              <Link to="/modpacks">
                Browse modpacks
                <ArrowRight />
              </Link>
            </Button>
          </section>
        </aside>
      </div>
      {creating && (
        <Suspense fallback={null}>
          <CreateServerDialog open={creating} onOpenChange={setCreating} />
        </Suspense>
      )}
      <Footer version={health.data?.version} />
    </>
  );
}

/** Host facts from Docker plus what the servers are using. */
export function HostPanel({ totals }: { totals: { memory: number; cpu: number; running: number } }) {
  const host = useQuery(hostQuery);
  const info = host.data;
  return (
    <Panel>
      <PanelHeader title="Host resources" icon={<Cpu />} />
      {host.isPending ? (
        <div className="h-24 animate-pulse rounded-md bg-surface" aria-busy />
      ) : !info?.dockerAvailable ? (
        <EmptyState compact icon={<Cpu />} title="Docker isn't reachable">
          {info?.error || "Host details appear once NetherNode can reach Docker."}
        </EmptyState>
      ) : (
        <>
          <div className="mb-5 flex items-center gap-[11px]">
            <span className="grid size-[37px] shrink-0 place-items-center rounded-md border border-primary/25 bg-nav-active text-nav-active-foreground">
              <Server className="size-[18px]" aria-hidden />
            </span>
            <div className="min-w-0">
              <p className="truncate font-[550] text-meta">{info.hostname || "This host"}</p>
              <p className="truncate text-meta text-text-muted">
                {info.operatingSystem} · Docker {info.dockerVersion}
              </p>
            </div>
          </div>
          <div className="flex flex-col gap-4">
            <ResourceMeter
              size="md"
              tone="cpu"
              label="CPU used by servers"
              value={`${Math.round(totals.cpu / Math.max(1, info.cpus))}% of ${info.cpus} cores`}
              percent={totals.cpu / Math.max(1, info.cpus)}
            />
            <ResourceMeter
              size="md"
              tone="memory"
              label="Memory used by servers"
              value={`${gb(totals.memory)} / ${gb(Number(info.memoryBytes))} GB`}
              percent={(totals.memory / Math.max(1, Number(info.memoryBytes))) * 100}
            />
          </div>
          <div className="mt-[18px] flex justify-between border-divider border-t pt-[13px] text-meta text-text-muted">
            <span>
              Running <b className="font-normal text-text-secondary">{totals.running}</b>
            </span>
            <span>Storage not measured yet</span>
          </div>
        </>
      )}
    </Panel>
  );
}
