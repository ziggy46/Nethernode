import { createClient } from "@connectrpc/connect";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { RefreshCw } from "lucide-react";
import { useId } from "react";
import { toast } from "sonner";

import { Panel, PanelHeader } from "@/components/page";
import { Switch } from "@/components/ui/switch";
import { NotificationService } from "@/gen/nethernode/v1/notifications_pb";
import { errorMessage, transport } from "@/lib/api";

const client = createClient(NotificationService, transport);
const key = ["notifications"] as const;

/** Whether the panel looks for new NetherNode releases. */
export default function UpdateCheckPanel() {
  const queryClient = useQueryClient();
  const id = useId();
  const settings = useQuery({ queryKey: key, queryFn: () => client.getNotifications({}) }).data;
  const set = useMutation({
    mutationFn: (enabled: boolean) => client.setUpdateCheck({ enabled }),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: key });
      void queryClient.invalidateQueries({ queryKey: ["notices"] });
    },
    onError: (e) => toast.error(errorMessage(e)),
  });
  if (!settings) return null;
  const on = set.isPending ? Boolean(set.variables) : settings.updateCheck;
  return (
    <Panel>
      <PanelHeader title="New versions" icon={<RefreshCw />} />
      <div className="flex items-start justify-between gap-5">
        <label htmlFor={id} className="text-label">
          <span className="block font-[550]">Tell me when a new version of NetherNode is out</span>
          <span className="mt-[3px] block text-meta text-text-muted">
            About twice a day the panel asks GitHub for the newest release (nothing about this install is
            sent), and the bell says when there's a newer one.
            {settings.updateCheck && settings.latestVersion && ` The newest is ${settings.latestVersion}.`}
          </span>
        </label>
        <Switch id={id} checked={on} disabled={set.isPending} onCheckedChange={(v) => set.mutate(v)} />
      </div>
    </Panel>
  );
}
