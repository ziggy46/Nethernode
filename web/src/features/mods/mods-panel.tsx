import { Lock } from "lucide-react";
import { lazy, Suspense } from "react";

import { EmptyState } from "@/components/page";
import type { Server } from "@/gen/nethernode/v1/servers_pb";
import { useCanManage } from "@/lib/session";

const ModsTab = lazy(() => import("./mods-tab"));

/** Loaders whose servers have a mods folder. */
const MODDED = ["fabric", "quilt", "forge", "neoforge", "modrinth", "curseforge"];

export function hasMods(server: Server): boolean {
  return MODDED.includes(server.loader);
}

/** The Mods section of the server workspace. Admins only. */
export function ModsPanel({ server }: { server: Server }) {
  const canManage = useCanManage();
  if (!canManage) {
    return (
      <div className="rounded-lg border border-divider border-dashed bg-surface-subtle">
        <EmptyState icon={<Lock />} title="Mods are for administrators">
          Ask an administrator if a mod needs adding or removing.
        </EmptyState>
      </div>
    );
  }
  return (
    <Suspense
      fallback={
        <div className="h-64 animate-pulse rounded-lg bg-surface-subtle" role="status" aria-busy>
          <span className="sr-only">Loading mods…</span>
        </div>
      }
    >
      <ModsTab server={server} />
    </Suspense>
  );
}
