import { useQuery } from "@tanstack/react-query";
import { Plus, Server as ServerIcon, TriangleAlert } from "lucide-react";
import { lazy, Suspense, useState } from "react";

import { EmptyState, Footer, PageHeader } from "@/components/page";
import { Button } from "@/components/ui/button";
import { errorMessage } from "@/lib/api";
import { healthQuery } from "@/lib/health";
import { useCanManage } from "@/lib/session";
import { useDocumentTitle } from "@/lib/use-document-title";

import { DockerNotice, useDockerAvailable } from "./docker-notice";
import { serversQuery } from "./queries";
import { ServerGrid } from "./server-grid";

const CreateServerDialog = lazy(() =>
  import("@/features/servers/create-server-dialog").then((m) => ({ default: m.CreateServerDialog })),
);

export function ServersPage() {
  useDocumentTitle("Servers");
  const servers = useQuery(serversQuery);
  const health = useQuery(healthQuery);
  const canManage = useCanManage();
  const docker = useDockerAvailable();
  const [creating, setCreating] = useState(false);

  return (
    <>
      <PageHeader
        eyebrow="Server fleet"
        title="Your servers"
        description="Every world, one place. Manage your Minecraft server fleet."
        actions={
          canManage && (
            <Button variant="primary" disabled={!docker} onClick={() => setCreating(true)}>
              <Plus />
              Create server
            </Button>
          )
        }
      />
      <DockerNotice />
      {servers.isPending ? (
        <div className="grid grid-cols-2 gap-4 below-sm:grid-cols-1" aria-busy>
          {[0, 1].map((i) => (
            <div
              key={i}
              className="h-[300px] animate-pulse rounded-lg border border-divider bg-surface-subtle"
            />
          ))}
        </div>
      ) : servers.isError ? (
        <EmptyState icon={<TriangleAlert />} title="Couldn't load servers">
          {errorMessage(servers.error)}
        </EmptyState>
      ) : servers.data.length === 0 ? (
        <div className="rounded-lg border border-divider border-dashed bg-surface-subtle">
          <EmptyState
            icon={<ServerIcon />}
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
        <ServerGrid servers={servers.data} />
      )}
      {creating && (
        <Suspense fallback={null}>
          <CreateServerDialog open={creating} onOpenChange={setCreating} />
        </Suspense>
      )}
      <Footer version={health.data?.version} />
    </>
  );
}
