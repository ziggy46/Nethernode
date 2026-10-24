import { Users } from "lucide-react";

import { Panel, PanelHeader } from "@/components/page";
import type { Server } from "@/gen/nethernode/v1/servers_pb";

import { useLiveStats } from "./server-card";

/**
 * Players for a Bedrock server. Its lists (allowlist, operators) live in
 * the server's console; NetherNode shows who's online.
 */
export function BedrockPlayers({ server }: { server: Server }) {
  const live = useLiveStats(server.id);
  const online = live?.players ?? null;
  return (
    <Panel>
      <PanelHeader title="Players" icon={<Users />} />
      <p className="text-label">
        {online === null
          ? "Online players show here while the server runs."
          : `${online} ${online === 1 ? "player" : "players"} online.`}
      </p>
      <p className="mt-2 max-w-[62ch] text-meta text-text-muted">
        Bedrock servers keep their own lists. Use the Console tab: <code>allowlist add Name</code>,{" "}
        <code>allowlist remove Name</code>, <code>op Name</code>, <code>kick Name</code>. Turn the allowlist
        on under Configuration → Game settings.
      </p>
    </Panel>
  );
}
