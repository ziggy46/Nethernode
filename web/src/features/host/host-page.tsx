import { useQueries, useQuery } from "@tanstack/react-query";
import { Network } from "lucide-react";

import { Footer, PageHeader, Panel, PanelHeader } from "@/components/page";
import { Badge, Dot } from "@/components/status-badge";
import { HostPanel } from "@/features/overview/overview-page";
import { formatBytes } from "@/features/servers/format";
import { hostQuery, type LiveStats, liveStatsKey, serversQuery } from "@/features/servers/queries";
import { ServerStatus } from "@/gen/nethernode/v1/servers_pb";
import { healthQuery } from "@/lib/health";
import { useDocumentTitle } from "@/lib/use-document-title";

export function HostPage() {
  useDocumentTitle("Host node");
  const host = useQuery(hostQuery);
  const health = useQuery(healthQuery);
  const servers = useQuery(serversQuery);
  const running = (servers.data ?? []).filter((s) => s.status === ServerStatus.RUNNING);
  const live = useQueries({
    queries: running.map((s) => ({
      queryKey: liveStatsKey(s.id),
      queryFn: (): LiveStats | null => null,
      enabled: false,
    })),
  });
  const totals = live.reduce(
    (t, q) => (q.data ? { memory: t.memory + q.data.memoryBytes, cpu: t.cpu + q.data.cpuPercent } : t),
    { memory: 0, cpu: 0 },
  );
  const info = host.data;

  return (
    <>
      <PageHeader
        eyebrow="Host node"
        title="A healthy home for your worlds."
        description="Monitor the machine behind your Minecraft servers."
      />
      <div className="grid grid-cols-[minmax(0,1fr)_260px] gap-5 below-lg:grid-cols-1">
        <div className="flex flex-col gap-[18px]">
          <HostPanel totals={{ ...totals, running: running.length }} />
          <Panel>
            <h2 className="mb-[15px] font-semibold text-section">Node details</h2>
            <dl className="text-meta">
              {[
                ["Hostname", info?.hostname || "—"],
                ["Operating system", info?.operatingSystem || "—"],
                ["Container runtime", info?.dockerAvailable ? `Docker ${info.dockerVersion}` : "Unavailable"],
                ["CPU", info?.cpus ? `${info.cpus} logical cores` : "—"],
                ["Total memory", info?.memoryBytes ? formatBytes(Number(info.memoryBytes)) : "—"],
              ].map(([label, value]) => (
                <div
                  key={label}
                  className="flex justify-between gap-3 border-divider border-b py-[11px] last:border-b-0"
                >
                  <dt className="text-text-muted">{label}</dt>
                  <dd className="text-right">{value}</dd>
                </div>
              ))}
            </dl>
          </Panel>
        </div>
        <Panel className="self-start">
          <PanelHeader title="Connection" icon={<Network />} />
          {info?.dockerAvailable ? (
            <Badge tone="success" icon={<Dot />}>
              Docker connected
            </Badge>
          ) : (
            <Badge tone="danger" icon={<Dot />}>
              Docker unreachable
            </Badge>
          )}
          <p className="mt-[17px] text-meta text-text-muted">
            {info?.dockerAvailable
              ? "NetherNode can create and run servers on this host."
              : info?.error || "Checking the connection…"}
          </p>
          <dl className="mt-2 text-meta">
            <div className="flex justify-between gap-3 border-divider border-t py-[11px]">
              <dt className="text-text-muted">NetherNode</dt>
              <dd>{health.data ? `v${health.data.version}` : "—"}</dd>
            </div>
          </dl>
        </Panel>
      </div>
      <Footer version={health.data?.version} />
    </>
  );
}
