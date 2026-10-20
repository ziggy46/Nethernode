import { Lock } from "lucide-react";
import { lazy, Suspense } from "react";

import { EmptyState } from "@/components/page";
import type { Server } from "@/gen/nethernode/v1/servers_pb";
import { useCanManage } from "@/lib/session";

const ModulesTab = lazy(() => import("./modules-tab"));

/** The Modules section of the server page. Admins only. */
export function ModulesPanel({ server }: { server: Server }) {
  const canManage = useCanManage();
  if (!canManage) {
    return (
      <div className="rounded-lg border border-divider border-dashed bg-surface-subtle">
        <EmptyState icon={<Lock />} title="Modules are for administrators">
          Ask an administrator if a module needs adding or changing.
        </EmptyState>
      </div>
    );
  }
  return (
    <Suspense
      fallback={
        <div className="h-64 animate-pulse rounded-lg bg-surface-subtle" role="status" aria-busy>
          <span className="sr-only">Loading modules…</span>
        </div>
      }
    >
      <ModulesTab server={server} />
    </Suspense>
  );
}
