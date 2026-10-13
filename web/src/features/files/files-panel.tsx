import { Lock } from "lucide-react";
import { lazy, Suspense } from "react";

import { EmptyState } from "@/components/page";
import type { Server } from "@/gen/nethernode/v1/servers_pb";
import { useCanManage } from "@/lib/session";

const FilesTab = lazy(() => import("./files-tab"));

/** The Files section of the server workspace. Admins only. */
export function FilesPanel({ server }: { server: Server }) {
  const canManage = useCanManage();
  if (!canManage) {
    return (
      <div className="rounded-lg border border-divider border-dashed bg-surface-subtle">
        <EmptyState icon={<Lock />} title="Files are for administrators">
          Ask an administrator if you need to change this server's files.
        </EmptyState>
      </div>
    );
  }
  return (
    <Suspense
      fallback={
        <div
          className="h-[400px] animate-pulse rounded-[9px] border border-border bg-surface-subtle"
          role="status"
          aria-busy
        >
          <span className="sr-only">Loading files…</span>
        </div>
      }
    >
      <FilesTab server={server} />
    </Suspense>
  );
}
