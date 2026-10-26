import { useQuery } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { CircleCheck, CircleDashed, ListChecks, TriangleAlert, X } from "lucide-react";
import { type ReactNode, useState } from "react";

import { Panel, PanelHeader } from "@/components/page";
import { Button } from "@/components/ui/button";
import { domainsQuery } from "@/features/domains/queries";
import { proxyClient } from "@/features/proxy/client";
import { listenersQuery } from "@/features/proxy/queries";
import { serversQuery } from "@/features/servers/queries";
import { cn } from "@/lib/utils";

import { joinAddress, listenerStatus } from "./getting-started";

const HIDDEN = "nn.guide.hidden";

function wasHidden(): boolean {
  try {
    return localStorage.getItem(HIDDEN) === "1";
  } catch {
    return false;
  }
}

type State = "done" | "todo" | "problem" | "manual";

function Step({
  state,
  title,
  action,
  children,
}: {
  state: State;
  title: string;
  action?: ReactNode;
  children: ReactNode;
}) {
  const Icon = state === "done" ? CircleCheck : state === "problem" ? TriangleAlert : CircleDashed;
  return (
    <li className="flex items-start gap-3 py-2.5">
      <Icon
        aria-hidden
        className={cn(
          "mt-0.5 size-4 shrink-0",
          state === "done" && "text-success",
          state === "problem" && "text-warning",
          (state === "todo" || state === "manual") && "text-text-muted",
        )}
      />
      <div className="min-w-0 flex-1">
        <p className="font-[550] text-label">
          {title}
          <span className="sr-only">
            {state === "done" ? " (done)" : state === "problem" ? " (needs attention)" : " (to do)"}
          </span>
        </p>
        <p className="mt-0.5 text-meta text-text-muted">{children}</p>
      </div>
      {action}
    </li>
  );
}

/**
 * A short checklist on the Overview for new installs: a server, a domain,
 * its DNS, the proxy, and a way to try joining from outside. Everything but
 * the last step is read from what the panel already knows; the last can only
 * be tried from another network, so it says how.
 */
export function GettingStartedPanel({
  onCreateServer,
  canCreate,
}: {
  onCreateServer: () => void;
  canCreate: boolean;
}) {
  const [hidden, setHidden] = useState(wasHidden);
  const servers = useQuery(serversQuery);
  const domains = useQuery(domainsQuery);
  const listeners = useQuery(listenersQuery);
  const domain = domains.data?.[0];
  const dns = useQuery({
    queryKey: ["proxy", "dns", domain],
    queryFn: () => proxyClient.checkDns({ hostname: `*.${domain}` }),
    enabled: !!domain,
    retry: false,
    staleTime: 60_000,
  });

  if (hidden || !servers.data || !domains.data || !listeners.data) return null;

  const serverCount = servers.data.length;
  const proxy = listenerStatus(listeners.data);
  const address = joinAddress(servers.data, domains.data);
  const port = proxy.kind === "off" ? 25565 : proxy.port;
  const hide = () => {
    setHidden(true);
    try {
      localStorage.setItem(HIDDEN, "1");
    } catch {
      // It comes back next visit; nothing is lost.
    }
  };

  return (
    <Panel className="mb-[30px] below-md:mb-6">
      <div className="relative">
        <PanelHeader title="Getting set up" icon={<ListChecks />} />
        <Button
          variant="ghost"
          size="mini"
          className="absolute -top-1 right-6"
          aria-label="Hide this guide"
          onClick={hide}
        >
          <X />
          Hide
        </Button>
      </div>
      <ol className="divide-y divide-divider">
        <Step
          state={serverCount > 0 ? "done" : "todo"}
          title="Create a server"
          action={
            serverCount === 0 && canCreate ? (
              <Button size="mini" variant="primary" onClick={onCreateServer}>
                Create server
              </Button>
            ) : undefined
          }
        >
          {serverCount > 0
            ? `${serverCount} server${serverCount === 1 ? "" : "s"} set up.`
            : "Pick a type and version; NetherNode downloads and starts it."}
        </Step>

        <Step
          state={domain ? "done" : "todo"}
          title="Add a domain (optional)"
          action={
            domain ? undefined : (
              <Button asChild size="mini">
                <Link to="/settings" search={{ section: "domains" }}>
                  Add a domain
                </Link>
              </Button>
            )
          }
        >
          {domain
            ? `Servers get names under ${domain}, like smp.${domain}.`
            : "With a domain you own, each server gets its own name to join with. Without one, players join by port."}
        </Step>

        {domain && (
          <Step
            state={dns.isPending ? "todo" : dns.isError || dns.data?.error ? "problem" : "done"}
            title="Point the domain at this network"
            action={
              <Button asChild size="mini">
                <Link to="/proxy">Records</Link>
              </Button>
            }
          >
            {dns.isPending
              ? "Checking…"
              : dns.isError || dns.data?.error
                ? `Names under ${domain} don't resolve yet. Add an A record for *.${domain} at your DNS provider; changes can take a few minutes.`
                : `Names under ${domain} point to ${dns.data.addresses.join(", ")}. Make sure that's this network's public address.`}
          </Step>
        )}

        <Step
          state={proxy.kind === "listening" ? "done" : proxy.kind === "failed" ? "problem" : "todo"}
          title="Let players in"
          action={
            proxy.kind === "listening" ? undefined : (
              <Button asChild size="mini">
                <Link to="/proxy">Open proxy</Link>
              </Button>
            )
          }
        >
          {proxy.kind === "listening"
            ? `The proxy is listening on port ${proxy.port}.`
            : proxy.kind === "failed"
              ? `The proxy couldn't open port ${proxy.port}: ${proxy.error}`
              : "The proxy isn't listening on any port. Turn a listener on."}
        </Step>

        <Step state="manual" title="Try joining from outside">
          NetherNode can't test this from inside your network. On a phone using mobile data, add{" "}
          <span className="font-mono text-text-secondary">
            {address ? (port === 25565 ? address : `${address}:${port}`) : `this network's address:${port}`}
          </span>{" "}
          under Multiplayer. If it doesn't connect, check that port {port} is forwarded on your router.
        </Step>
      </ol>
    </Panel>
  );
}
