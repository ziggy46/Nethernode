import { useQueries, useQuery } from "@tanstack/react-query";
import { Network } from "lucide-react";

import { PageHeader, Panel, PanelHeader } from "@/components/page";
import { Badge, Dot } from "@/components/status-badge";
import { HostPanel } from "@/features/overview/overview-page";
import { hostQuery, type LiveStats, liveStatsKey, serversQuery } from "@/features/servers/queries";
import { ServerStatus } from "@/gen/nethernode/v1/servers_pb";
import { useDocumentTitle } from "@/lib/use-document-title";

export function HostPage() {
  useDocumentTitle("This machine");
  const host = useQuery(hostQuery);
  const servers = useQuery(serversQuery);
  // Only this machine's servers: others report on their own machines.
  const running = (servers.data ?? []).filter(
    (s) => s.status === ServerStatus.RUNNING && s.nodeId === "local",
  );
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
        eyebrow="This machine"
        title="A healthy home for your worlds."
        description="The machine this panel runs on, and how much of it your servers use. Other machines are on the Nodes page."
      />
      <div className="grid grid-cols-[minmax(0,1fr)_260px] gap-5 below-lg:grid-cols-1">
        <div className="flex flex-col gap-[18px]">
          <HostPanel totals={{ ...totals, running: running.length }} />
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
        </Panel>
      </div>
    </>
  );
}
