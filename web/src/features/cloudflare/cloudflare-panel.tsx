import { createClient } from "@connectrpc/connect";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { CircleAlert, CircleCheck, CircleDashed, Cloud, ExternalLink } from "lucide-react";
import { useId, useState } from "react";
import { toast } from "sonner";

import { ConfirmDialog } from "@/components/confirm-dialog";
import { Panel, PanelHeader } from "@/components/page";
import { Badge } from "@/components/status-badge";
import { Button } from "@/components/ui/button";
import { ErrorAlert, Input } from "@/components/ui/field";
import { CloudflareService } from "@/gen/nethernode/v1/cloudflare_pb";
import { errorMessage, transport } from "@/lib/api";

import { cloudflareKey } from "./queries";

const client = createClient(CloudflareService, transport);

const dateFormat = new Intl.DateTimeFormat(undefined, { dateStyle: "medium", timeStyle: "short" });

function List({ title, items, tone }: { title: string; items: string[]; tone?: "warning" }) {
  if (items.length === 0) return null;
  return (
    <div className="mt-3">
      <p className={tone === "warning" ? "font-[550] text-label text-warning" : "font-[550] text-label"}>
        {title}
      </p>
      <ul className="mt-1 flex flex-col gap-0.5 text-meta text-text-secondary">
        {items.map((i) => (
          <li key={i} className="font-mono break-all">
            {i}
          </li>
        ))}
      </ul>
    </div>
  );
}

/** Cloudflare: a tunnel and DNS for modules' addresses, managed by NetherNode. Owners and administrators. */
export default function CloudflarePanel() {
  const queryClient = useQueryClient();
  const tokenId = useId();
  const status = useQuery({
    queryKey: cloudflareKey,
    queryFn: () => client.getCloudflare({}),
    refetchInterval: 15_000,
  });
  const [token, setToken] = useState("");
  const [preview, setPreview] = useState<{ changes: string[]; conflicts: string[] } | null>(null);
  const [confirmOff, setConfirmOff] = useState(false);
  const [confirmDisconnect, setConfirmDisconnect] = useState(false);
  const refresh = () => queryClient.invalidateQueries({ queryKey: cloudflareKey });
  const connect = useMutation({
    mutationFn: () => client.connectCloudflare({ token }),
    onSuccess: () => {
      setToken("");
      toast.success("Connected to Cloudflare. The tunnel's connector is starting.");
      void refresh();
    },
  });
  const look = useMutation({
    mutationFn: () => client.previewCloudflare({}),
    onSuccess: (r) => setPreview({ changes: r.changes, conflicts: r.conflicts }),
    onError: (e) => toast.error(errorMessage(e)),
  });
  const dns = useMutation({
    mutationFn: (enabled: boolean) => client.setCloudflareDns({ enabled }),
    onSuccess: (r, enabled) => {
      setPreview(null);
      setConfirmOff(false);
      toast.success(
        enabled
          ? `DNS management is on${r.changes.length ? ` (${r.changes.length} changes made)` : ""}.`
          : "DNS management is off; NetherNode's records were removed.",
      );
      void refresh();
    },
    onError: (e) => toast.error(errorMessage(e)),
  });
  const disconnect = useMutation({
    mutationFn: () => client.disconnectCloudflare({}),
    onSuccess: () => {
      setConfirmDisconnect(false);
      toast.success("Disconnected from Cloudflare.");
      void refresh();
    },
  });

  const data = status.data;
  if (status.isError) {
    return (
      <Panel>
        <PanelHeader title="Cloudflare" icon={<Cloud />} />
        <ErrorAlert>{errorMessage(status.error)}</ErrorAlert>
      </Panel>
    );
  }
  if (!data) return <div className="h-40 animate-pulse rounded-lg bg-surface-subtle" aria-busy />;

  if (!data.connected) {
    return (
      <Panel>
        <PanelHeader title="Cloudflare" icon={<Cloud />} />
        <p className="mb-3 text-label text-text-secondary">
          With a Cloudflare API token, NetherNode makes a Cloudflare Tunnel, runs it, and adds modules' own
          addresses (like map.mc1.example.com) to it and to your DNS, with HTTPS from Cloudflare and no port
          to forward. Minecraft connections don't use it: players keep joining as they do now.
        </p>
        <p className="font-[550] text-label">Make a token</p>
        <p className="mt-[3px] mb-3 text-meta text-text-muted">
          In Cloudflare, under My Profile › API Tokens › Create Token › Custom token, give it Account ›
          Cloudflare Tunnel › Edit, Zone › Zone › Read, and Zone › DNS › Edit for your domain.{" "}
          <a
            href="https://dash.cloudflare.com/profile/api-tokens"
            target="_blank"
            rel="noreferrer noopener"
            className="inline-flex items-center gap-1 underline underline-offset-2"
          >
            API tokens
            <ExternalLink className="size-3" aria-hidden />
          </a>
        </p>
        <form
          className="flex flex-wrap items-center gap-2"
          onSubmit={(e) => {
            e.preventDefault();
            if (token.trim()) connect.mutate();
          }}
        >
          <label htmlFor={tokenId} className="sr-only">
            Cloudflare API token
          </label>
          <Input
            id={tokenId}
            type="password"
            autoComplete="off"
            placeholder="API token"
            className="max-w-sm font-mono"
            value={token}
            onChange={(e) => setToken(e.target.value)}
          />
          <Button type="submit" variant="primary" disabled={!token.trim() || connect.isPending}>
            {connect.isPending ? "Connecting…" : "Connect"}
          </Button>
        </form>
        {connect.error && <ErrorAlert className="mt-3">{errorMessage(connect.error)}</ErrorAlert>}
      </Panel>
    );
  }

  const connector =
    data.connectorStatus === "running" ? (
      <Badge tone="success" icon={<CircleCheck className="size-3" />}>
        Tunnel connected
      </Badge>
    ) : data.connectorStatus === "absent" ? (
      <Badge tone="muted" icon={<CircleDashed className="size-3" />}>
        Connector not running
      </Badge>
    ) : (
      <Badge tone="danger" icon={<CircleAlert className="size-3" />}>
        Connector {data.connectorStatus}
      </Badge>
    );

  return (
    <Panel>
      <PanelHeader title="Cloudflare" icon={<Cloud />} />
      <div className="mb-4 flex flex-wrap items-center gap-x-3 gap-y-1 border-divider border-b pb-4 text-label">
        {connector}
        <span className="text-text-secondary">
          {data.accountName} · token {data.tokenHint} · {data.zones.join(", ") || "no domains"}
        </span>
      </div>
      {data.connectorMessage && (
        <p className="mb-3 font-mono text-meta text-danger">{data.connectorMessage}</p>
      )}
      {data.lastError && <ErrorAlert className="mb-3">{data.lastError}</ErrorAlert>}

      <p className="font-[550] text-label">
        DNS and routes for modules' addresses {data.dns ? "(on)" : "(off)"}
      </p>
      <p className="mt-[3px] text-meta text-text-muted">
        {data.dns
          ? `Open modules' addresses in ${data.zones.join(", ")} lead to the tunnel. Checked again every ten minutes and whenever an address changes${data.lastSyncMs ? `; last at ${dateFormat.format(new Date(Number(data.lastSyncMs)))}` : ""}.`
          : "Off until you've seen what it would change. NetherNode only ever changes DNS records it made itself."}
      </p>
      {data.dns ? (
        <>
          <List title="Leading to the tunnel" items={data.routed} />
          <List title="Can't be managed" items={data.conflicts} tone="warning" />
          <List title="Outside these domains (set up by hand)" items={data.outside} />
          <List title="Last changes" items={data.changes} />
          <div className="mt-4 flex gap-2">
            <Button size="mini" onClick={() => setConfirmOff(true)}>
              Turn off
            </Button>
          </div>
        </>
      ) : (
        <>
          {preview && (
            <div className="mt-3 rounded-md border border-border p-3">
              <List
                title={
                  preview.changes.length ? "Turning it on would:" : "Turning it on changes nothing right now."
                }
                items={preview.changes}
              />
              <List title="Can't be managed" items={preview.conflicts} tone="warning" />
              <p className="mt-3 text-meta text-text-muted">
                A name above a module's address that only answered through a wildcard record gets its own copy
                of it, so players can still reach that server. Copies follow the wildcard and go with the last
                address under them.
              </p>
            </div>
          )}
          <div className="mt-4 flex gap-2">
            <Button size="mini" disabled={look.isPending} onClick={() => look.mutate()}>
              {look.isPending ? "Looking…" : "Preview"}
            </Button>
            <Button
              size="mini"
              variant="primary"
              disabled={!preview || dns.isPending}
              onClick={() => dns.mutate(true)}
            >
              {dns.isPending ? "Turning on…" : "Turn on"}
            </Button>
          </div>
        </>
      )}

      <div className="mt-6 border-divider border-t pt-4">
        <Button size="mini" variant="ghost" onClick={() => setConfirmDisconnect(true)}>
          Disconnect Cloudflare
        </Button>
      </div>
      <ConfirmDialog
        open={confirmOff}
        onOpenChange={setConfirmOff}
        title="Turn off DNS management?"
        description="NetherNode removes the DNS records it added and the tunnel's routes, so modules' addresses stop working through Cloudflare. Records you made yourself stay."
        confirmLabel="Turn off"
        pending={dns.isPending}
        onConfirm={() => dns.mutate(false)}
      />
      <ConfirmDialog
        open={confirmDisconnect}
        onOpenChange={setConfirmDisconnect}
        title="Disconnect Cloudflare?"
        description="NetherNode removes the DNS records it added, deletes its tunnel, stops the connector, and forgets the token."
        confirmLabel="Disconnect"
        pending={disconnect.isPending}
        error={disconnect.error ? errorMessage(disconnect.error) : undefined}
        onConfirm={() => disconnect.mutate()}
      />
    </Panel>
  );
}
