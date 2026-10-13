import { lazy, Suspense } from "react";

import type { Server } from "@/gen/nethernode/v1/servers_pb";

const PlayersView = lazy(() => import("./players-view").then((m) => ({ default: m.PlayersView })));

/** The Players section of the server workspace. */
export function PlayersPanel({ server }: { server: Server }) {
  return (
    <Suspense
      fallback={
        <div
          className="h-[320px] animate-pulse rounded-lg border border-divider bg-surface-subtle"
          role="status"
          aria-busy
        >
          <span className="sr-only">Loading players…</span>
        </div>
      }
    >
      <PlayersView serverId={server.id} serverName={server.name} />
    </Suspense>
  );
}
