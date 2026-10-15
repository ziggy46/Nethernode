import { lazy, Suspense } from "react";

import type { Server } from "@/gen/nethernode/v1/servers_pb";

const BackupsTab = lazy(() => import("./backups-tab"));

/** The Backups section of the server page, loaded when opened. */
export function BackupsPanel({ server }: { server: Server }) {
  return (
    <Suspense
      fallback={
        <div className="h-64 animate-pulse rounded-lg bg-surface-subtle" role="status" aria-busy>
          <span className="sr-only">Loading backups…</span>
        </div>
      }
    >
      <BackupsTab server={server} />
    </Suspense>
  );
}
