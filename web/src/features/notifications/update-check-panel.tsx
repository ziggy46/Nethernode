import { createClient } from "@connectrpc/connect";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Download, RefreshCw } from "lucide-react";
import { useEffect, useId, useState } from "react";
import { toast } from "sonner";

import { ConfirmDialog } from "@/components/confirm-dialog";
import { Panel, PanelHeader } from "@/components/page";
import { Button } from "@/components/ui/button";
import { Switch } from "@/components/ui/switch";
import { NotificationService } from "@/gen/nethernode/v1/notifications_pb";
import { errorMessage, transport } from "@/lib/api";

const client = createClient(NotificationService, transport);
const key = ["notifications"] as const;

/** New NetherNode versions: whether to look for them, and installing one. */
export default function UpdateCheckPanel() {
  const queryClient = useQueryClient();
  const id = useId();
  const settings = useQuery({ queryKey: key, queryFn: () => client.getNotifications({}) }).data;
  const [confirm, setConfirm] = useState(false);
  const [installing, setInstalling] = useState<string | null>(null);
  const refresh = () => {
    void queryClient.invalidateQueries({ queryKey: key });
    void queryClient.invalidateQueries({ queryKey: ["notices"] });
  };
  const set = useMutation({
    mutationFn: (enabled: boolean) => client.setUpdateCheck({ enabled }),
    onSuccess: refresh,
    onError: (e) => toast.error(errorMessage(e)),
  });
  const check = useMutation({
    mutationFn: () => client.checkForUpdate({}),
    onSuccess: (r) => {
      refresh();
      toast.success(
        r.updateAvailable ? `NetherNode ${r.latestVersion} is available.` : "You're on the newest version.",
      );
    },
    onError: (e) => toast.error(errorMessage(e)),
  });
  const update = useMutation({
    mutationFn: () => client.updatePanel({}),
    onSuccess: (r) => {
      setConfirm(false);
      setInstalling(r.version);
    },
  });

  if (installing) return <Installing version={installing} />;
  if (!settings) return null;
  const on = set.isPending ? Boolean(set.variables) : settings.updateCheck;
  const latest = settings.latestVersion;
  return (
    <Panel>
      <PanelHeader title="New versions" icon={<RefreshCw />} />
      <div className="mb-4 flex flex-wrap items-center justify-between gap-3 border-divider border-b pb-4">
        <div className="text-label">
          <p className="font-[550]">
            {settings.updateAvailable
              ? `NetherNode ${latest} is available`
              : latest
                ? "You're on the newest version"
                : `This panel runs NetherNode ${settings.currentVersion}`}
          </p>
          <p className="mt-[3px] text-meta text-text-muted">
            {settings.updateAvailable
              ? `This panel runs ${settings.currentVersion}. Updating restarts the panel for a minute or so; servers keep running, and nodes on other machines follow on their own.`
              : latest
                ? `${settings.currentVersion} is the newest release.`
                : "Check now to see if there's a newer one."}
          </p>
        </div>
        <div className="flex gap-2">
          <Button size="mini" disabled={check.isPending} onClick={() => check.mutate()}>
            <RefreshCw />
            {check.isPending ? "Checking…" : "Check now"}
          </Button>
          {settings.updateAvailable && (
            <Button size="mini" variant="primary" onClick={() => setConfirm(true)}>
              <Download />
              Update now
            </Button>
          )}
        </div>
      </div>
      <div className="flex items-start justify-between gap-5">
        <label htmlFor={id} className="text-label">
          <span className="block font-[550]">Tell me when a new version is out</span>
          <span className="mt-[3px] block text-meta text-text-muted">
            About twice a day the panel asks GitHub for the newest release (nothing about this install is
            sent), and the bell says when there's a newer one.
          </span>
        </label>
        <Switch id={id} checked={on} disabled={set.isPending} onCheckedChange={(v) => set.mutate(v)} />
      </div>
      <ConfirmDialog
        open={confirm}
        onOpenChange={(open) => {
          setConfirm(open);
          if (!open) update.reset();
        }}
        title={`Update NetherNode to ${latest}?`}
        description="The panel downloads the new version and restarts, which takes a minute or so. Minecraft servers keep running and players stay connected to them; the panel is unavailable meanwhile."
        confirmLabel={update.isPending ? "Starting…" : "Update now"}
        pending={update.isPending}
        error={update.error ? errorMessage(update.error) : undefined}
        onConfirm={() => update.mutate()}
      />
    </Panel>
  );
}

/** Waits for the new version to answer, then reloads the page. */
function Installing({ version }: { version: string }) {
  const [waited, setWaited] = useState(0);
  useEffect(() => {
    const started = Date.now();
    const timer = setInterval(async () => {
      setWaited(Math.round((Date.now() - started) / 1000));
      try {
        const health = (await (await fetch("/healthz", { cache: "no-store" })).json()) as {
          version?: string;
        };
        if (health.version === version) window.location.reload();
      } catch {
        // Restarting: not answering yet.
      }
    }, 2000);
    return () => clearInterval(timer);
  }, [version]);
  return (
    <Panel>
      <PanelHeader title="New versions" icon={<RefreshCw className="animate-spin" />} />
      <p className="font-[550] text-label">Updating to NetherNode {version}…</p>
      <p className="mt-[3px] text-meta text-text-muted" role="status">
        {waited < 180
          ? "The panel is restarting. This page reloads when it's back."
          : "This is taking longer than expected. Check the panel's logs (docker compose logs) if it doesn't come back."}
      </p>
    </Panel>
  );
}
