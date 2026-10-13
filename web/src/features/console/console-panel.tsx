import { Lock, SquareTerminal } from "lucide-react";
import { lazy, Suspense } from "react";

import { EmptyState } from "@/components/page";
import { ServerStatusBadge } from "@/components/status-badge";
import type { Server } from "@/gen/nethernode/v1/servers_pb";
import { useCanModerate } from "@/lib/session";

import { useDockerAvailable } from "../servers/docker-notice";

const ConsoleView = lazy(() => import("./console-view"));

/** The console section of the server workspace. */
export function ConsolePanel({ server }: { server: Server }) {
  const canModerate = useCanModerate();
  const docker = useDockerAvailable();

  if (!canModerate) {
    return (
      <Frame server={server}>
        <EmptyState compact icon={<Lock />} title="The console needs a moderator role">
          Ask an administrator for access to read the server's output and send commands.
        </EmptyState>
      </Frame>
    );
  }
  if (!docker) {
    return (
      <Frame server={server}>
        <EmptyState compact icon={<SquareTerminal />} title="The console is unavailable">
          NetherNode can't reach Docker, so it can't read the server's output.
        </EmptyState>
      </Frame>
    );
  }
  return (
    <Suspense
      fallback={
        <Frame server={server} bare>
          <div className="h-[407px] animate-pulse below-md:h-[367px]" role="status" aria-busy>
            <span className="sr-only">Loading the console…</span>
          </div>
        </Frame>
      }
    >
      <ConsoleView server={server} />
    </Suspense>
  );
}

/** The console's chrome without a terminal, for loading and unavailable states. */
function Frame({ server, bare, children }: { server: Server; bare?: boolean; children: React.ReactNode }) {
  return (
    <section
      aria-label="Server console"
      className="overflow-hidden rounded-[9px] border border-border bg-console"
    >
      <div className="flex items-center gap-3 border-border border-b bg-surface px-[17px] py-[13px] text-meta">
        <SquareTerminal className="size-4" aria-hidden />
        Server console
        <span className="ml-auto">
          <ServerStatusBadge status={server.status} />
        </span>
      </div>
      {bare ? (
        children
      ) : (
        <div className="flex min-h-[220px] flex-col items-center justify-center p-8">{children}</div>
      )}
    </section>
  );
}
