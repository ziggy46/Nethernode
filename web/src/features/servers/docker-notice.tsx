import { useQuery } from "@tanstack/react-query";
import { TriangleAlert } from "lucide-react";

import { hostQuery } from "./queries";

/** Explains why server management is off when Docker can't be reached. */
export function DockerNotice() {
  const host = useQuery(hostQuery);
  if (!host.data || host.data.dockerAvailable) return null;
  return (
    <div
      role="alert"
      className="mb-6 flex items-start gap-2.5 rounded-lg border border-warning/30 bg-warning-subtle px-4 py-3 text-label text-warning"
    >
      <TriangleAlert className="mt-0.5 size-4 shrink-0" aria-hidden />
      <div>
        <p className="font-semibold">Docker isn't available</p>
        <p className="mt-0.5 text-meta">
          NetherNode can't reach Docker ({host.data.error}), so servers can't be created, started, or stopped.
          Check that Docker is running and that NetherNode can access its socket.
        </p>
      </div>
    </div>
  );
}

/** Whether server lifecycle actions can run right now. */
export function useDockerAvailable(): boolean {
  const host = useQuery(hostQuery);
  return host.data?.dockerAvailable ?? true;
}
