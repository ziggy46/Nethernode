import { useQuery } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { ChevronRight, Copy, Loader, Play, Server as ServerIcon, SquareTerminal, Users } from "lucide-react";
import { toast } from "sonner";

import { ResourceMeter } from "@/components/resource-meter";
import { ServerStatusBadge } from "@/components/status-badge";
import { Button } from "@/components/ui/button";
import { type Server, ServerStatus } from "@/gen/nethernode/v1/servers_pb";
import { useCanManage } from "@/lib/session";

import { formatUptime, gb, loaderName, versionLabel } from "./format";
import { type LiveStats, liveStatsKey } from "./queries";
import { useServerAction } from "./use-server-actions";
import { WorldBanner } from "./world-banner";

/** Reads the latest live sample pushed by `server_stats` events. */
export function useLiveStats(id: string): LiveStats | undefined {
  return (
    useQuery<LiveStats | null>({
      queryKey: liveStatsKey(id),
      queryFn: () => null,
      enabled: false,
    }).data ?? undefined
  );
}

export function subtitle(server: Server, now: number): string {
  switch (server.status) {
    case ServerStatus.RUNNING:
      return server.lastStartedAtMs
        ? `Up for ${formatUptime(now - Number(server.lastStartedAtMs))}`
        : "Running";
    case ServerStatus.SLEEPING:
      return "Asleep · wakes when a player joins";
    case ServerStatus.INSTALLING:
      return "Installing…";
    case ServerStatus.STARTING:
      return "Starting…";
    case ServerStatus.STOPPING:
      return "Stopping…";
    case ServerStatus.CRASHED:
      return "Crashed";
    default:
      return "Ready when you are";
  }
}

export async function copyAddress(address: string) {
  try {
    await navigator.clipboard.writeText(address);
    toast.success("Server address copied.");
  } catch {
    toast(`Address: ${address}`, { description: "Couldn't copy automatically; select and copy it." });
  }
}

/** Server card, per the UI reference's anatomy. */
export function ServerCard({ server, now }: { server: Server; now: number }) {
  const live = useLiveStats(server.id);
  const canManage = useCanManage();
  const action = useServerAction(server.id, server.name);
  const running = server.status === ServerStatus.RUNNING;
  const busy = [ServerStatus.INSTALLING, ServerStatus.STARTING, ServerStatus.STOPPING].includes(
    server.status,
  );
  const memoryLimit = live?.memoryLimitBytes ?? (server.settings?.memoryMb ?? 0) * 1024 * 1024;
  const address = server.hostnames[0];

  return (
    <article className="group min-w-0 overflow-hidden rounded-lg border border-border bg-surface-subtle transition duration-200 hover:-translate-y-0.5 hover:border-primary/40 motion-reduce:hover:translate-y-0">
      <div className="relative h-[95px] overflow-hidden min-[1500px]:h-[125px] below-sm:h-[115px]">
        <WorldBanner theme={server.settings?.cardTheme ?? 0} />
        <div className="pointer-events-none absolute inset-0 bg-linear-to-t from-(--world-fade) to-transparent" />
        <div className="absolute top-3 left-[13px]">
          <ServerStatusBadge status={server.status} nodeOffline={server.nodeOffline} />
        </div>
        <Button
          asChild
          variant="ghost"
          size="icon"
          className="absolute top-[11px] right-3 size-[27px] border-text-primary/15 bg-background/65 text-text-primary"
        >
          <Link to="/servers/$serverId" params={{ serverId: server.id }} aria-label={`Open ${server.name}`}>
            <ChevronRight className="size-4" />
          </Link>
        </Button>
      </div>

      <div className="px-[17px] pt-[15px] pb-[14px] min-[1500px]:p-5 below-xl:p-[13px]">
        <div className="mb-[3px] flex items-center gap-2">
          <Link
            to="/servers/$serverId"
            params={{ serverId: server.id }}
            className="truncate font-semibold text-body tracking-[-0.2px] hover:text-primary min-[1500px]:text-[16px]"
          >
            {server.name}
          </Link>
          <ServerIcon className="ml-auto size-[13px] shrink-0 text-text-muted" aria-hidden />
        </div>
        <p className="flex flex-wrap items-center gap-1.5 text-meta text-text-muted">
          <span className="max-w-[16ch] truncate rounded-xs bg-surface-elevated px-[5px] py-px text-text-secondary">
            {server.modpack?.title ?? loaderName(server.loader)}
          </span>
          <span>{versionLabel(server.mcVersion)}</span>
          <span aria-hidden>·</span>
          <span>{subtitle(server, now)}</span>
          {server.nodeId !== "local" && (
            <>
              <span aria-hidden>·</span>
              <span>on {server.nodeName}</span>
            </>
          )}
        </p>

        <div className="mt-[13px] flex min-h-5 items-center gap-[5px] font-mono text-meta text-text-secondary">
          {address ? (
            <>
              <span className="truncate">{address}</span>
              <button
                type="button"
                onClick={() => void copyAddress(address)}
                className="inline-flex rounded-xs p-0.5 text-text-muted hover:text-text-primary"
                aria-label={`Copy ${address}`}
              >
                <Copy className="size-3" />
              </button>
            </>
          ) : (
            <Link to="/proxy" className="font-sans text-text-muted underline-offset-2 hover:underline">
              No hostname yet · add a route
            </Link>
          )}
        </div>

        <div className="mt-[14px] grid grid-cols-2 gap-4 min-[1500px]:mt-5">
          <ResourceMeter
            label="CPU"
            tone="cpu"
            value={live && running ? `${Math.round(live.cpuPercent)}%` : "—"}
            percent={live && running ? live.cpuPercent / Math.max(1, server.settings?.cpuLimit ?? 1) : null}
          />
          <ResourceMeter
            label="Memory"
            tone="memory"
            value={
              live && running
                ? `${gb(live.memoryBytes)} / ${gb(memoryLimit)} GB`
                : `— / ${gb(memoryLimit)} GB`
            }
            percent={live && running && memoryLimit ? (live.memoryBytes / memoryLimit) * 100 : null}
          />
        </div>

        <div className="mt-[15px] flex items-center justify-between gap-2 border-divider border-t pt-[11px] min-[1500px]:mt-5">
          <span className="flex items-center gap-[5px] text-meta text-text-secondary">
            <Users className="size-[13px] text-text-muted" aria-hidden />
            {running && server.playersOnline !== undefined ? (
              <>
                <b className="font-semibold text-lava">{server.playersOnline}</b> / {server.playersMax ?? "—"}{" "}
                players
              </>
            ) : (
              <>{server.settings?.maxPlayers ?? 0} player slots</>
            )}
          </span>
          {running ? (
            <Button asChild size="mini">
              <Link to="/servers/$serverId" params={{ serverId: server.id }}>
                <SquareTerminal />
                Console
              </Link>
            </Button>
          ) : busy ? (
            <Button size="mini" disabled>
              <Loader className="animate-spin motion-reduce:animate-none" />
              {subtitle(server, now)}
            </Button>
          ) : (
            canManage && (
              <Button
                size="mini"
                variant="accent"
                disabled={action.isPending}
                onClick={() => action.mutate("start")}
              >
                <Play />
                {server.status === ServerStatus.SLEEPING ? "Wake server" : "Start server"}
              </Button>
            )
          )}
        </div>
      </div>
    </article>
  );
}
