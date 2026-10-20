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
import { cn } from "@/lib/utils";

const client = createClient(NotificationService, transport);
const key = ["notifications"] as const;

const CHANNELS = [
  { value: "stable", label: "Stable" },
  { value: "edge", label: "Edge" },
] as const;

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
  const channel = useMutation({
    mutationFn: (value: string) => client.setUpdateChannel({ channel: value }),
    onSuccess: refresh,
    onError: (e) => toast.error(errorMessage(e)),
  });
  const check = useMutation({
    mutationFn: () => client.checkForUpdate({}),
    onSuccess: (r) => {
      refresh();
      toast.success(
        !r.updateAvailable
          ? "You're on the newest version."
          : r.updateIsSwitch
            ? `NetherNode ${r.latestVersion} is the stable release to go back to.`
            : `NetherNode ${r.latestVersion} is available.`,
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
  const current = channel.isPending ? channel.variables : settings.updateChannel || "stable";
  const switching = settings.updateAvailable && settings.updateIsSwitch;
  return (
    <Panel>
      <PanelHeader title="New versions" icon={<RefreshCw />} />
      <div className="mb-4 flex flex-wrap items-center justify-between gap-3 border-divider border-b pb-4">
        <div className="text-label">
          <p className="font-[550]">
            {switching
              ? `NetherNode ${latest} is the stable release to go back to`
              : settings.updateAvailable
                ? `NetherNode ${latest} is available`
                : latest
                  ? "You're on the newest version"
                  : `This panel runs NetherNode ${settings.currentVersion}`}
          </p>
          <p className="mt-[3px] text-meta text-text-muted">
            {switching
              ? `This panel runs the edge build ${settings.currentVersion}. Going back restarts the panel for a minute or so; servers keep running, and nodes on other machines follow.`
              : settings.updateAvailable
                ? `This panel runs ${settings.currentVersion}. Updating restarts the panel for a minute or so; servers keep running, and nodes on other machines follow on their own.`
                : latest
                  ? `${settings.currentVersion} is the newest ${current === "edge" ? "build" : "release"}.`
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
              {switching ? "Go back to stable" : "Update now"}
            </Button>
          )}
        </div>
      </div>
      <div className="mb-4 flex items-center justify-between gap-5 border-divider border-b pb-4 below-sm:flex-col below-sm:items-start">
        <div className="min-w-0 text-label">
          <p className="font-[550]" id={`${id}-channel`}>
            Update channel
          </p>
          <p className="mt-[3px] text-meta text-text-muted">
            {current === "edge"
              ? "Edge builds: new features as soon as they're built, before they're released. They can have rough edges. Switch back to Stable any time."
              : "Stable releases only."}
          </p>
        </div>
        <div
          role="radiogroup"
          aria-labelledby={`${id}-channel`}
          className="flex shrink-0 rounded-md border border-border-strong bg-field p-0.5"
        >
          {CHANNELS.map((c) => (
            <label
              key={c.value}
              className={cn(
                "flex cursor-pointer items-center gap-1.5 rounded-[5px] px-2.5 py-1 text-label transition-colors duration-(--nn-transition)",
                "has-focus-visible:outline-2 has-focus-visible:outline-primary",
                current === c.value
                  ? "bg-nav-active text-nav-active-foreground"
                  : "text-text-secondary hover:bg-button-quiet-hover hover:text-text-primary",
              )}
            >
              <input
                type="radio"
                name="update-channel"
                value={c.value}
                checked={current === c.value}
                disabled={channel.isPending}
                onChange={() => channel.mutate(c.value)}
                className="sr-only"
              />
              {c.label}
            </label>
          ))}
        </div>
      </div>
      <div className="flex items-start justify-between gap-5">
        <label htmlFor={id} className="text-label">
          <span className="block font-[550]">Tell me when a new version is out</span>
          <span className="mt-[3px] block text-meta text-text-muted">
            About twice a day the panel asks GitHub for the newest release, and once a day it asks Modrinth or
            CurseForge for newer versions of your servers' modpacks (nothing about this install is sent). The
            bell says when there's something newer.
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
        title={switching ? `Go back to NetherNode ${latest}?` : `Update NetherNode to ${latest}?`}
        description={
          switching
            ? "The panel goes back to the newest stable release and restarts, which takes a minute or so. Minecraft servers keep running. Anything that only edge builds have (such as modules) stops being managed until you switch to Edge again; its settings and files are kept."
            : "The panel downloads the new version and restarts, which takes a minute or so. Minecraft servers keep running and players stay connected to them; the panel is unavailable meanwhile."
        }
        confirmLabel={update.isPending ? "Starting…" : switching ? "Go back" : "Update now"}
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
