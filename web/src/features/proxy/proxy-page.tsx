import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  ArrowRight,
  Globe,
  Network,
  Pencil,
  Plus,
  RadioTower,
  Trash2,
  TriangleAlert,
  Users,
} from "lucide-react";
import { useEffect, useState } from "react";
import { toast } from "sonner";

import { ConfirmDialog } from "@/components/confirm-dialog";
import { EmptyState, Footer, PageHeader, Panel, PanelHeader, SectionHeading } from "@/components/page";
import { Badge, Dot, ServerStatusBadge } from "@/components/status-badge";
import { Button } from "@/components/ui/button";
import { Switch } from "@/components/ui/switch";
import { type Listener, ListenerState, type Route, RouteKind } from "@/gen/nethernode/v1/proxy_pb";
import type { Server } from "@/gen/nethernode/v1/servers_pb";
import { errorMessage, proxyClient } from "@/lib/api";
import { healthQuery } from "@/lib/health";
import { useCanManage } from "@/lib/session";
import { useDocumentTitle } from "@/lib/use-document-title";
import { cn } from "@/lib/utils";

import { dnsRecords, MINECRAFT_PORT, wildcardNeedsPort } from "./dns";
import { ListenerDialog, PROXY_PROTOCOL_WARNING } from "./listener-dialog";
import { connectionsQuery, listenersQuery, routesQuery, serversQuery } from "./queries";
import { RouteDialog } from "./route-dialog";

export function ProxyPage() {
  useDocumentTitle("Proxy & routes");
  const canManage = useCanManage();
  const health = useQuery(healthQuery);
  const [routeDialog, setRouteDialog] = useState<{ open: boolean; route?: Route }>({ open: false });

  return (
    <>
      <PageHeader
        eyebrow="Smart routing"
        title="One address. Every world."
        description="Route players to the right server with a hostname."
        actions={
          canManage && (
            <Button variant="primary" onClick={() => setRouteDialog({ open: true })}>
              <Plus />
              Add route
            </Button>
          )
        }
      />
      <ProxyHealth />
      <RoutesSection onEdit={(route) => setRouteDialog({ open: true, route })} canManage={canManage} />
      <div className="mt-6 grid grid-cols-2 gap-[18px] below-lg:grid-cols-1">
        <ListenersPanel canManage={canManage} />
        <ConnectionsPanel />
      </div>
      <DnsPanel />
      <RouteDialog
        open={routeDialog.open}
        route={routeDialog.route}
        onOpenChange={(open) => setRouteDialog((d) => ({ ...d, open }))}
      />
      <Footer version={health.data?.version} />
    </>
  );
}

/** Tinted icon tile used on proxy cards. */
function IconTile({ children }: { children: React.ReactNode }) {
  return (
    <span className="grid size-[37px] shrink-0 place-items-center rounded-md border border-primary/25 bg-nav-active text-nav-active-foreground [&_svg]:size-[18px]">
      {children}
    </span>
  );
}

function ProxyHealth() {
  const listeners = useQuery(listenersQuery);
  let tone: "success" | "warning" | "danger" | "muted" = "muted";
  let label = "Checking…";
  let detail = "Loading listener status.";

  if (listeners.isError) {
    tone = "danger";
    label = "Unknown";
    detail = errorMessage(listeners.error);
  } else if (listeners.data) {
    const list = listeners.data;
    const listening = list.filter((l) => l.state === ListenerState.LISTENING);
    const failed = list.filter((l) => l.state === ListenerState.FAILED);
    if (list.some((l) => l.state === ListenerState.PROXY_DISABLED)) {
      tone = "muted";
      label = "Disabled";
      detail = "The Smart Proxy is turned off in the NetherNode configuration.";
    } else if (list.length === 0) {
      tone = "muted";
      label = "No listeners";
      detail = "Add a listener so players have a port to connect to.";
    } else if (failed.length > 0) {
      tone = listening.length > 0 ? "warning" : "danger";
      label = listening.length > 0 ? "Degraded" : "Down";
      detail = `${failed.length} of ${list.length} listener${list.length === 1 ? "" : "s"} couldn't start.`;
    } else if (listening.length === 0) {
      tone = "muted";
      label = "Paused";
      detail = "Every listener is turned off.";
    } else {
      tone = "success";
      label = "Healthy";
      const ports = [...new Set(listening.map((l) => l.port))].join(", ");
      detail = `Listening on port${listening.length === 1 ? "" : "s"} ${ports} · Hostname routing enabled`;
    }
  }

  return (
    <Panel className="mb-6 flex items-center gap-3">
      <IconTile>
        <Network />
      </IconTile>
      <div className="min-w-0">
        <h2 className="font-semibold text-[13px]">NetherNode proxy</h2>
        <p className="mt-0.5 text-meta text-text-muted">{detail}</p>
      </div>
      <Badge tone={tone} icon={<Dot />} className="ml-auto">
        {label}
      </Badge>
    </Panel>
  );
}

function RoutesSection({ onEdit, canManage }: { onEdit: (route: Route) => void; canManage: boolean }) {
  const routes = useQuery(routesQuery);
  const servers = useQuery(serversQuery);
  const connections = useQuery(connectionsQuery);
  const byId = new Map(servers.data?.map((s) => [s.id, s]));
  const bySlug = new Map(servers.data?.map((s) => [s.slug, s]));
  // Players (not status pings) on each route right now.
  const playing = new Map<string, number>();
  for (const c of connections.data ?? []) {
    if (c.username) playing.set(c.route, (playing.get(c.route) ?? 0) + 1);
  }

  return (
    <section aria-labelledby="routes-heading">
      <div id="routes-heading">
        <SectionHeading title="Routes" count={routes.data?.length} caption="Most specific match wins." />
      </div>
      {routes.isPending ? (
        <div className="flex flex-col gap-3" aria-busy>
          {[0, 1].map((i) => (
            <div
              key={i}
              className="h-[78px] animate-pulse rounded-[9px] border border-divider bg-surface-subtle"
            />
          ))}
        </div>
      ) : routes.isError ? (
        <Panel>
          <EmptyState compact icon={<TriangleAlert />} title="Couldn't load routes">
            {errorMessage(routes.error)}
          </EmptyState>
        </Panel>
      ) : routes.data.length === 0 ? (
        <div className="rounded-lg border border-divider border-dashed bg-surface-subtle">
          <EmptyState icon={<Globe />} title="No routes yet">
            Routes send players to a server based on the address they type, like play.example.com.{" "}
            {servers.data?.length ? "Add one to let players in." : "Add one once you have a server."}
          </EmptyState>
        </div>
      ) : (
        <ul className="flex flex-col gap-3">
          {routes.data.map((route) => (
            <RouteCard
              key={route.id}
              route={route}
              players={connections.data ? (playing.get(route.hostname) ?? 0) : undefined}
              server={route.serverId ? byId.get(route.serverId) : undefined}
              slugTargets={route.serverId ? undefined : bySlug.size}
              canManage={canManage}
              onEdit={() => onEdit(route)}
            />
          ))}
        </ul>
      )}
    </section>
  );
}

const KIND_LABEL: Record<RouteKind, string> = {
  [RouteKind.UNSPECIFIED]: "Route",
  [RouteKind.EXACT]: "Incoming Minecraft connection",
  [RouteKind.WILDCARD]: "Any subdomain",
  [RouteKind.DEFAULT]: "Any address no other route matches",
};

function RouteCard({
  route,
  players,
  server,
  slugTargets,
  canManage,
  onEdit,
}: {
  route: Route;
  players: number | undefined;
  server: Server | undefined;
  slugTargets: number | undefined;
  canManage: boolean;
  onEdit: () => void;
}) {
  const queryClient = useQueryClient();
  const [confirmOpen, setConfirmOpen] = useState(false);
  const dns = useMutation({ mutationFn: () => proxyClient.checkDns({ hostname: route.hostname }) });
  const remove = useMutation({
    mutationFn: () => proxyClient.deleteRoute({ id: route.id }),
    onSuccess: async () => {
      setConfirmOpen(false);
      // Server cards list the hostnames that reach each server.
      await Promise.all([
        queryClient.invalidateQueries({ queryKey: ["proxy", "routes"] }),
        queryClient.invalidateQueries({ queryKey: ["servers"] }),
      ]);
      toast.success(`Route ${route.hostname} removed.`);
    },
  });

  return (
    <li className="rounded-[9px] border border-border bg-surface-subtle p-5 below-md:p-4">
      {/*
        Desktop: icon | hostname | → | destination | status | actions.
        Phone: icon | hostname | status, then destination, then actions.
      */}
      <div className="grid grid-cols-[auto_minmax(0,1fr)_auto_minmax(0,1fr)_auto_auto] items-center gap-5 below-md:grid-cols-[auto_minmax(0,1fr)_auto] below-md:gap-x-3 below-md:gap-y-2">
        <IconTile>
          <Network />
        </IconTile>
        <div className="min-w-0">
          <p className="truncate font-mono font-semibold text-[13px]">{route.hostname}</p>
          <p className="mt-1 text-meta text-text-muted">
            {KIND_LABEL[route.kind]}
            {players !== undefined && players > 0 && (
              <span className="text-success"> · {players} playing now</span>
            )}
          </p>
        </div>
        <ArrowRight className="size-[18px] text-primary below-md:hidden" aria-label="routes to" />
        <div className="min-w-0 below-md:col-span-2 below-md:col-start-2 below-md:row-start-2">
          {route.serverId ? (
            <>
              <p className="truncate font-semibold text-[13px]">
                <span className="hidden text-primary below-md:inline" aria-hidden>
                  →{" "}
                </span>
                {route.serverName || "Unknown server"}
              </p>
              <p className="mt-1 text-meta text-text-muted">
                {server ? `Internal port ${server.internalPort}` : "Server details unavailable"}
              </p>
            </>
          ) : (
            <>
              <p className="font-semibold text-[13px]">
                <span className="hidden text-primary below-md:inline" aria-hidden>
                  →{" "}
                </span>
                Server matching the subdomain
              </p>
              <p className="mt-1 text-meta text-text-muted">
                {slugTargets
                  ? `${slugTargets} server${slugTargets === 1 ? "" : "s"} by slug`
                  : "No servers yet"}
              </p>
            </>
          )}
        </div>
        <div className="below-md:col-start-3 below-md:row-start-1">
          {server && <ServerStatusBadge status={server.status} />}
        </div>
        <div className="flex items-center gap-1 below-md:col-span-2 below-md:col-start-2 below-md:row-start-3">
          {route.kind !== RouteKind.DEFAULT && (
            <Button
              variant="ghost"
              size="mini"
              onClick={() => dns.mutate()}
              disabled={dns.isPending}
              aria-label={`Check DNS for ${route.hostname}`}
            >
              <Globe />
              {dns.isPending ? "Checking…" : "DNS"}
            </Button>
          )}
          {canManage && (
            <>
              <Button
                variant="ghost"
                size="icon"
                className="size-[27px]"
                aria-label={`Edit ${route.hostname}`}
                onClick={onEdit}
              >
                <Pencil className="size-3.5" />
              </Button>
              <Button
                variant="ghost"
                size="icon"
                className="size-[27px] hover:not-disabled:text-danger"
                aria-label={`Delete ${route.hostname}`}
                onClick={() => setConfirmOpen(true)}
              >
                <Trash2 className="size-3.5" />
              </Button>
            </>
          )}
        </div>
      </div>
      {(dns.isSuccess || dns.isError) && (
        <p
          role="status"
          className={cn(
            "mt-3 border-divider border-t pt-3 text-meta",
            dns.isError || dns.data?.error ? "text-warning" : "text-text-secondary",
          )}
        >
          {dns.isError
            ? `DNS check failed: ${errorMessage(dns.error)}`
            : dns.data.error
              ? `${dns.data.resolvedName}: ${dns.data.error}`
              : `${dns.data.resolvedName} resolves to ${dns.data.addresses.join(", ")}. Make sure that's this machine's public address.`}
        </p>
      )}
      <ConfirmDialog
        open={confirmOpen}
        onOpenChange={(open) => {
          if (!open) remove.reset();
          setConfirmOpen(open);
        }}
        title="Remove this route?"
        description={`Players connecting with ${route.hostname} will no longer reach ${route.serverName || "this server"}. Players already connected stay connected.`}
        confirmLabel="Remove route"
        pending={remove.isPending}
        error={remove.isError ? errorMessage(remove.error) : undefined}
        onConfirm={() => remove.mutate()}
      />
    </li>
  );
}

const LISTENER_STATE: Record<ListenerState, { tone: "success" | "danger" | "muted"; label: string }> = {
  [ListenerState.UNSPECIFIED]: { tone: "muted", label: "Unknown" },
  [ListenerState.LISTENING]: { tone: "success", label: "Listening" },
  [ListenerState.FAILED]: { tone: "danger", label: "Failed" },
  [ListenerState.DISABLED]: { tone: "muted", label: "Off" },
  [ListenerState.PROXY_DISABLED]: { tone: "muted", label: "Proxy off" },
};

function ListenersPanel({ canManage }: { canManage: boolean }) {
  const listeners = useQuery(listenersQuery);
  const [dialog, setDialog] = useState<{ open: boolean; listener?: Listener }>({ open: false });

  return (
    <Panel>
      <div className="mb-[17px] flex items-center justify-between gap-3">
        <h2 className="font-semibold text-label tracking-[-0.3px]">Listeners</h2>
        {canManage && (
          <Button variant="subtle" size="mini" onClick={() => setDialog({ open: true })}>
            <Plus />
            Add listener
          </Button>
        )}
      </div>
      {listeners.isPending ? (
        <div className="h-12 animate-pulse rounded-md bg-surface" aria-busy />
      ) : listeners.isError ? (
        <EmptyState compact icon={<TriangleAlert />} title="Couldn't load listeners">
          {errorMessage(listeners.error)}
        </EmptyState>
      ) : listeners.data.length === 0 ? (
        <EmptyState compact icon={<RadioTower />} title="No listeners">
          Players need at least one port to connect to.
        </EmptyState>
      ) : (
        <ul className="flex flex-col">
          {listeners.data.map((listener) => (
            <ListenerRow
              key={listener.id}
              listener={listener}
              canManage={canManage}
              onEdit={() => setDialog({ open: true, listener })}
            />
          ))}
        </ul>
      )}
      <ListenerDialog
        open={dialog.open}
        listener={dialog.listener}
        onOpenChange={(open) => setDialog((d) => ({ ...d, open }))}
      />
    </Panel>
  );
}

function ListenerRow({
  listener,
  canManage,
  onEdit,
}: {
  listener: Listener;
  canManage: boolean;
  onEdit: () => void;
}) {
  const queryClient = useQueryClient();
  const [confirmOpen, setConfirmOpen] = useState(false);
  const state = LISTENER_STATE[listener.state];
  const address = `${listener.bindAddr.includes(":") ? `[${listener.bindAddr}]` : listener.bindAddr}:${listener.port}`;

  const toggle = useMutation({
    mutationFn: (enabled: boolean) =>
      proxyClient.upsertListener({
        id: listener.id,
        bindAddr: listener.bindAddr,
        port: listener.port,
        enabled,
        proxyProtocol: listener.proxyProtocol,
      }),
    onSettled: () => queryClient.invalidateQueries({ queryKey: ["proxy", "listeners"] }),
    onError: (error) => toast.error(`Couldn't update ${address}. ${errorMessage(error)}`),
  });
  const remove = useMutation({
    mutationFn: () => proxyClient.deleteListener({ id: listener.id }),
    onSuccess: async () => {
      setConfirmOpen(false);
      await queryClient.invalidateQueries({ queryKey: ["proxy"] });
      toast.success(`Listener ${address} removed.`);
    },
  });

  return (
    <li className="border-divider border-b py-3 first:pt-0 last:border-b-0 last:pb-0">
      <div className="flex flex-wrap items-center gap-2.5">
        <span className="font-mono text-label">{address}</span>
        <Badge tone={state.tone} icon={<Dot />}>
          {state.label}
        </Badge>
        {listener.proxyProtocol && <Badge tone="muted">PROXY protocol</Badge>}
        {canManage && (
          <div className="ml-auto flex items-center gap-1">
            <Switch
              checked={listener.enabled}
              disabled={toggle.isPending || listener.state === ListenerState.PROXY_DISABLED}
              onCheckedChange={(enabled) => toggle.mutate(enabled)}
              aria-label={`Listener ${address} enabled`}
              className="mr-1.5"
            />
            <Button
              variant="ghost"
              size="icon"
              className="size-[27px]"
              aria-label={`Edit ${address}`}
              onClick={onEdit}
            >
              <Pencil className="size-3.5" />
            </Button>
            <Button
              variant="ghost"
              size="icon"
              className="size-[27px] hover:not-disabled:text-danger"
              aria-label={`Delete ${address}`}
              onClick={() => setConfirmOpen(true)}
            >
              <Trash2 className="size-3.5" />
            </Button>
          </div>
        )}
      </div>
      {listener.error && <p className="mt-1.5 text-meta text-danger">{listener.error}</p>}
      {listener.proxyProtocol && <p className="mt-1.5 text-meta text-text-muted">{PROXY_PROTOCOL_WARNING}</p>}
      <ConfirmDialog
        open={confirmOpen}
        onOpenChange={(open) => {
          if (!open) remove.reset();
          setConfirmOpen(open);
        }}
        title="Delete this listener?"
        description={`New players can no longer connect on ${address}, and routes limited to it are deleted. Players already connected stay connected.`}
        confirmLabel="Delete listener"
        pending={remove.isPending}
        error={remove.isError ? errorMessage(remove.error) : undefined}
        onConfirm={() => remove.mutate()}
      />
    </li>
  );
}

/** Re-renders every `ms` so relative times stay current. */
function useNow(ms: number): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const id = setInterval(() => setNow(Date.now()), ms);
    return () => clearInterval(id);
  }, [ms]);
  return now;
}

function formatDuration(ms: number): string {
  const minutes = Math.floor(ms / 60_000);
  if (minutes < 1) return "just now";
  if (minutes < 60) return `${minutes}m`;
  const hours = Math.floor(minutes / 60);
  return `${hours}h ${minutes % 60}m`;
}

function ConnectionsPanel() {
  const connections = useQuery(connectionsQuery);
  const now = useNow(30_000);

  return (
    <Panel aria-live="polite">
      <PanelHeader title="Connected through the proxy" icon={<Users />} />
      {connections.isPending ? (
        <div className="h-12 animate-pulse rounded-md bg-surface" aria-busy />
      ) : connections.isError ? (
        <EmptyState compact icon={<TriangleAlert />} title="Couldn't load connections">
          {errorMessage(connections.error)}
        </EmptyState>
      ) : connections.data.length === 0 ? (
        <EmptyState compact icon={<Users />} title="No players connected">
          Players appear here while they're connected through the proxy.
        </EmptyState>
      ) : (
        <ul className="flex flex-col">
          {connections.data.map((c) => (
            <li
              key={`${c.clientIp}-${c.startedAtMs}-${c.username ?? ""}`}
              className="border-divider border-b py-2.5 first:pt-0 last:border-b-0 last:pb-0"
            >
              <p className="text-label">
                <span className="font-semibold">{c.username ?? "Unknown player"}</span>
                <span className="text-text-muted"> on </span>
                {c.serverName || "a server"}
              </p>
              <p className="mt-0.5 text-meta text-text-muted">
                <span className="font-mono">{c.hostname || "(no hostname)"}</span> ·{" "}
                <span className="font-mono">{c.clientIp}</span> ·{" "}
                {formatDuration(now - Number(c.startedAtMs))}
              </p>
            </li>
          ))}
        </ul>
      )}
    </Panel>
  );
}

/** The DNS records players need, so the routes above can be reached. */
function DnsPanel() {
  const routes = useQuery(routesQuery);
  const listeners = useQuery(listenersQuery);
  if (!routes.data || !listeners.data) return null;
  const records = dnsRecords(routes.data, listeners.data);
  if (records.length === 0) return null;
  return (
    <Panel className="mt-6">
      <PanelHeader title="DNS records to create" icon={<Globe />} />
      <p className="mb-3 text-meta text-text-muted">
        Add these at your domain's DNS provider. Use this machine's public address (or your router's, with
        port {MINECRAFT_PORT} forwarded here), then use a route's DNS button to check it.
      </p>
      <div className="overflow-x-auto rounded-md border border-divider">
        <table className="w-full min-w-[480px] text-meta">
          <thead className="bg-surface text-left text-text-muted">
            <tr>
              {["Type", "Name", "Value"].map((h) => (
                <th key={h} scope="col" className="border-divider border-b px-3 py-2 font-medium">
                  {h}
                </th>
              ))}
            </tr>
          </thead>
          <tbody>
            {records.map((r) => (
              <tr key={`${r.type}-${r.name}-${r.value}`} className="border-divider border-b last:border-b-0">
                <td className="px-3 py-2 font-mono">{r.type === "A" ? "A / AAAA" : r.type}</td>
                <td className="px-3 py-2 font-mono">{r.name}</td>
                <td className={cn("px-3 py-2", r.type === "SRV" ? "font-mono" : "text-text-secondary")}>
                  {r.value}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
      {wildcardNeedsPort(routes.data, listeners.data) && (
        <p className="mt-3 text-meta text-warning">
          Wildcard names can't have SRV records, so players joining through a wildcard route need to type the
          port too.
        </p>
      )}
    </Panel>
  );
}
