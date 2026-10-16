import { createClient } from "@connectrpc/connect";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { ChartNoAxesColumn } from "lucide-react";
import { useId } from "react";
import { toast } from "sonner";

import { Panel, PanelHeader } from "@/components/page";
import { ErrorAlert } from "@/components/ui/field";
import { Switch } from "@/components/ui/switch";
import { InstallCountService } from "@/gen/nethernode/v1/install_count_pb";
import { errorMessage, transport } from "@/lib/api";
import { ago } from "@/lib/time";
import { useNow } from "@/lib/use-now";

const client = createClient(InstallCountService, transport);
const key = ["install-count"] as const;

/** The opt-in active installs count. Owners only. */
export default function InstallCountPanel() {
  const queryClient = useQueryClient();
  const now = useNow();
  const switchId = useId();
  const status = useQuery({ queryKey: key, queryFn: () => client.getInstallCount({}) });
  const set = useMutation({
    mutationFn: (enabled: boolean) => client.setInstallCount({ enabled }),
    onSuccess: (_, enabled) => {
      toast.success(
        enabled
          ? "This install will be counted. Thank you!"
          : "Counting is off and the install ID is deleted.",
      );
      void queryClient.invalidateQueries({ queryKey: key });
    },
    onError: (e) => toast.error(errorMessage(e)),
  });
  const data = status.data;

  return (
    <Panel>
      <PanelHeader title="Count this install" icon={<ChartNoAxesColumn />} />
      {status.isError ? (
        <ErrorAlert>{errorMessage(status.error)}</ErrorAlert>
      ) : !data ? (
        <div className="h-20 animate-pulse rounded-md bg-surface" role="status" aria-busy />
      ) : (
        <>
          <div className="flex items-start justify-between gap-5">
            <div className="min-w-0">
              <label htmlFor={switchId} className="font-[550] text-label">
                Add this install to the public count of active NetherNode installs
              </label>
              <p className="mt-[3px] text-meta text-text-muted">
                Off unless you turn it on. Once a day NetherNode sends a random ID made on this machine and
                its version, nothing else: no names, addresses, servers, or players. Turning it off deletes
                the ID.
              </p>
            </div>
            <Switch
              id={switchId}
              checked={data.enabled}
              disabled={set.isPending}
              onCheckedChange={(on) => set.mutate(on)}
            />
          </div>
          {data.enabled && (
            <dl className="mt-4 space-y-2 border-divider border-t pt-3 text-meta">
              <div>
                <dt className="text-text-muted">Sent once a day</dt>
                <dd className="mt-1 break-all rounded-sm bg-surface px-2 py-1.5 font-mono text-text-secondary">
                  {data.payload}
                </dd>
              </div>
              <div className="flex flex-wrap justify-between gap-2">
                <dt className="text-text-muted">To</dt>
                <dd className="break-all font-mono text-text-secondary">{data.url}</dd>
              </div>
              <div className="flex justify-between gap-2">
                <dt className="text-text-muted">Last sent</dt>
                <dd className={data.lastError ? "text-warning" : "text-text-secondary"}>
                  {data.lastError
                    ? `Not yet: ${data.lastError}`
                    : data.lastSentMs !== undefined
                      ? ago(data.lastSentMs, now)
                      : "Within the hour"}
                </dd>
              </div>
            </dl>
          )}
        </>
      )}
    </Panel>
  );
}
