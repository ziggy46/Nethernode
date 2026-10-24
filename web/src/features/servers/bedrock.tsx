import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Plug } from "lucide-react";

import { Panel, PanelHeader } from "@/components/page";
import { Button } from "@/components/ui/button";
import { CopyChip } from "@/components/ui/copy-chip";
import { ErrorAlert } from "@/components/ui/field";
import { proxyClient } from "@/features/proxy/client";
import type { Server } from "@/gen/nethernode/v1/servers_pb";
import { errorMessage } from "@/lib/api";
import { useCanManage } from "@/lib/session";

import { listenersQuery, routesQuery } from "../proxy/queries";

/** Whether a server runs Bedrock Dedicated Server (UDP, no Java, no mods). */
export const isBedrock = (loader: string) => loader === "bedrock";

/** Bedrock's own default port; each further server takes the next free one. */
export const BEDROCK_FIRST_PORT = 19132;

/** The lowest port from [`BEDROCK_FIRST_PORT`] that no listener uses. */
export function nextFreePort(used: { port: number }[]): number {
  const taken = new Set(used.map((l) => l.port));
  let port = BEDROCK_FIRST_PORT;
  while (taken.has(port)) port++;
  return port;
}

/**
 * The UDP port players use for a Bedrock server: the port of the listener
 * made for it. `undefined` for other servers, or until it has one.
 */
export function useBedrockPort(server: Pick<Server, "id" | "loader">): number | undefined {
  const bedrock = isBedrock(server.loader);
  const listeners = useQuery({ ...listenersQuery, enabled: bedrock });
  const routes = useQuery({ ...routesQuery, enabled: bedrock });
  if (!bedrock) return undefined;
  const route = routes.data?.find((r) => r.serverId === server.id && r.hostname === "*" && r.listenerId);
  return listeners.data?.find((l) => l.id === route?.listenerId)?.port;
}

/** A Bedrock server's port, as a chip that copies it (the header's address). */
export function BedrockPortChip({ server }: { server: Pick<Server, "id" | "loader"> }) {
  const port = useBedrockPort(server);
  return port ? <CopyChip text={String(port)} mono className="text-label" /> : null;
}

/** How a player adds a Bedrock server: shown beside its controls. */
export function BedrockJoin({ server }: { server: Server }) {
  const port = useBedrockPort(server);
  const canManage = useCanManage();
  const queryClient = useQueryClient();
  const listeners = useQuery(listenersQuery);
  const open = useMutation({
    mutationFn: () =>
      proxyClient.addServerPort({ serverId: server.id, port: nextFreePort(listeners.data ?? []) }),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ["proxy"] }),
  });
  return (
    <Panel>
      <PanelHeader title="Join from Bedrock" icon={<Plug />} />
      {port ? (
        <>
          <p className="mb-2 text-meta text-text-muted">
            In Minecraft: Play → Servers → Add Server. Enter this machine's address and the port:
          </p>
          <CopyChip text={String(port)} mono className="text-label" />
          <p className="mt-2 text-meta text-text-muted">
            Port {port} is UDP. Friends outside your network need it forwarded on your router. Consoles (Xbox,
            PlayStation, Switch) can't add servers by address.
          </p>
        </>
      ) : (
        <>
          {open.isError && <ErrorAlert>{errorMessage(open.error)}</ErrorAlert>}
          <p className="text-meta text-text-muted">
            This server has no port of its own yet, so players can't reach it.
          </p>
          {canManage && (
            <Button
              className="mt-2.5 w-full"
              disabled={open.isPending || !listeners.data}
              onClick={() => open.mutate()}
            >
              <Plug />
              Open port {nextFreePort(listeners.data ?? [])}
            </Button>
          )}
        </>
      )}
    </Panel>
  );
}
