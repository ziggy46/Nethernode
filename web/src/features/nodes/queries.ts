import { createClient } from "@connectrpc/connect";
import { queryOptions } from "@tanstack/react-query";

import { type JoinCode, NodeService } from "@/gen/nethernode/v1/nodes_pb";
import { transport } from "@/lib/api";

export const nodeClient = createClient(NodeService, transport);

export const nodesKey = ["nodes"] as const;

/** Every machine; refetched when a node changes (`node_changed`). */
export const nodesQuery = queryOptions({
  queryKey: nodesKey,
  queryFn: () => nodeClient.listNodes({}),
});

/** The command that starts an agent and joins it to this panel. */
export function joinCommand(join: JoinCode, origin: string): string {
  return [
    "docker run -d --name nethernode-agent --restart unless-stopped --network host \\",
    "  -v /var/run/docker.sock:/var/run/docker.sock -v nethernode-agent:/data \\",
    `  ${join.image} agent --join ${origin} --code ${join.token}`,
  ].join("\n");
}
