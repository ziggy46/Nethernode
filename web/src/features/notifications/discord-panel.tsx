import { createClient } from "@connectrpc/connect";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { BellRing, Send } from "lucide-react";
import { useId, useState } from "react";
import { toast } from "sonner";

import { Panel, PanelHeader } from "@/components/page";
import { Button } from "@/components/ui/button";
import { ErrorAlert, Input } from "@/components/ui/field";
import { Switch } from "@/components/ui/switch";
import { type NotificationEvents, NotificationService } from "@/gen/nethernode/v1/notifications_pb";
import { errorMessage, transport } from "@/lib/api";
import { ago } from "@/lib/time";
import { useNow } from "@/lib/use-now";

const client = createClient(NotificationService, transport);
const key = ["notifications"] as const;

type Toggle = keyof Omit<NotificationEvents, "$typeName" | "$unknown">;

const EVENTS: { key: Toggle; title: string; description: string }[] = [
  { key: "crash", title: "Crashes", description: "A server stops unexpectedly." },
  { key: "startStop", title: "Starts and stops", description: "Including restarts and scheduled ones." },
  {
    key: "playerJoin",
    title: "Players joining",
    description: "Joins a few seconds apart are sent together.",
  },
  { key: "backupFailed", title: "Failed backups", description: "Scheduled or manual." },
];

/** Discord webhook settings. Administrators and owners. */
export default function DiscordPanel() {
  const queryClient = useQueryClient();
  const now = useNow();
  const urlId = useId();
  const settings = useQuery({ queryKey: key, queryFn: () => client.getNotifications({}) });
  const [url, setUrl] = useState("");
  const save = useMutation({
    mutationFn: (change: { webhookUrl?: string; events: Partial<Record<Toggle, boolean>> }) =>
      client.updateNotifications(change),
    onSuccess: (_, change) => {
      if (change.webhookUrl !== undefined) {
        setUrl("");
        toast.success(change.webhookUrl ? "Webhook saved." : "Webhook removed.");
      }
      void queryClient.invalidateQueries({ queryKey: key });
    },
    onError: (e) => toast.error(errorMessage(e)),
  });
  const test = useMutation({
    mutationFn: () => client.sendTestNotification({}),
    onSuccess: () => toast.success("Sent. Check the Discord channel."),
    onSettled: () => void queryClient.invalidateQueries({ queryKey: key }),
  });
  const data = settings.data;
  const events = data?.events;
  const current: Record<Toggle, boolean> = {
    crash: events?.crash ?? false,
    startStop: events?.startStop ?? false,
    playerJoin: events?.playerJoin ?? false,
    backupFailed: events?.backupFailed ?? false,
  };

  return (
    <Panel>
      <PanelHeader title="Discord notifications" icon={<BellRing />} />
      {settings.isError ? (
        <ErrorAlert>{errorMessage(settings.error)}</ErrorAlert>
      ) : !data ? (
        <div className="h-32 animate-pulse rounded-md bg-surface" role="status" aria-busy />
      ) : (
        <>
          <label htmlFor={urlId} className="font-[550] text-label">
            Webhook URL
          </label>
          <p className="mt-[3px] mb-3 text-meta text-text-muted">
            {data.webhookHint
              ? `Posting to the webhook ending ${data.webhookHint}. Paste a new one to replace it.`
              : "In Discord, open a channel's settings, then Integrations › Webhooks › New Webhook › Copy Webhook URL."}
          </p>
          <form
            className="flex gap-2 below-sm:flex-col"
            onSubmit={(e) => {
              e.preventDefault();
              if (url.trim()) save.mutate({ webhookUrl: url.trim(), events: current });
            }}
          >
            <Input
              id={urlId}
              type="password"
              autoComplete="off"
              spellCheck={false}
              placeholder="https://discord.com/api/webhooks/…"
              className="font-mono"
              value={url}
              onChange={(e) => setUrl(e.target.value)}
            />
            <Button type="submit" disabled={!url.trim() || save.isPending}>
              Save
            </Button>
          </form>
          {data.webhookHint && (
            <div className="mt-3 flex flex-wrap items-center gap-2">
              <Button size="mini" disabled={test.isPending} onClick={() => test.mutate()}>
                <Send />
                {test.isPending ? "Sending…" : "Send a test"}
              </Button>
              <Button
                size="mini"
                variant="ghost"
                disabled={save.isPending}
                onClick={() => save.mutate({ webhookUrl: "", events: current })}
              >
                Remove webhook
              </Button>
              <span className="text-meta text-text-muted">
                {data.lastSentMs !== undefined
                  ? `Last sent ${ago(data.lastSentMs, now)}`
                  : "Nothing sent yet"}
              </span>
            </div>
          )}
          {(test.error || data.lastError) && (
            <ErrorAlert className="mt-3 mb-0">
              {test.error ? errorMessage(test.error) : data.lastError}
            </ErrorAlert>
          )}
          <ul className="mt-4 divide-y divide-divider border-divider border-t">
            {EVENTS.map((e) => (
              <li key={e.key} className="flex items-center justify-between gap-4 py-3">
                <div>
                  <p className="text-label">{e.title}</p>
                  <p className="text-meta text-text-muted">{e.description}</p>
                </div>
                <Switch
                  checked={current[e.key]}
                  disabled={save.isPending}
                  aria-label={`Notify about ${e.title.toLowerCase()}`}
                  onCheckedChange={(on) => save.mutate({ events: { ...current, [e.key]: on } })}
                />
              </li>
            ))}
          </ul>
        </>
      )}
    </Panel>
  );
}
