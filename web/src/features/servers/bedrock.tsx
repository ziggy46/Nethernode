import { Plug } from "lucide-react";

import { Panel, PanelHeader } from "@/components/page";
import { CopyChip } from "@/components/ui/copy-chip";
import type { Server } from "@/gen/nethernode/v1/servers_pb";

/** Whether a server runs Bedrock Dedicated Server (no Java, no mods). */
export const isBedrock = (loader: string) => loader === "bedrock";

/** The signalling port of the first Bedrock server; each next one takes the next number. */
const FIRST_PORT = 19132;
/** How many Bedrock servers get ports of their own. */
const SLOTS = 64;
/** The first UDP port of the first server's range; each server has 32. */
const UDP_BASE = 19200;
const UDP_PORTS = 32;

/**
 * The UDP ports a Bedrock server's players' game traffic uses, from the TCP
 * port they first connect to. The same arithmetic as the server's
 * (`bedrock_udp_start` in the orchestrator).
 */
export function bedrockUdpRange(tcpPort: number): [number, number] {
  const slot = tcpPort - FIRST_PORT;
  const start =
    slot >= 0 && slot < SLOTS ? UDP_BASE + slot * UDP_PORTS : Math.min(tcpPort + 100, 65535 - UDP_PORTS);
  return [start, start + UDP_PORTS - 1];
}

/** A Bedrock server's port, as a chip that copies it (the header's address). */
export function BedrockPortChip({ server }: { server: Pick<Server, "internalPort"> }) {
  return <CopyChip text={String(server.internalPort)} mono className="text-label" />;
}

/** How a player reaches a Bedrock server, and what to forward on the router. */
export function BedrockJoin({ server }: { server: Server }) {
  const port = server.internalPort;
  const [from, to] = bedrockUdpRange(port);
  return (
    <Panel>
      <PanelHeader title="Join from Bedrock" icon={<Plug />} />
      <p className="mb-2 text-meta text-text-muted">
        In Minecraft: Play → Servers → Add Server. Enter this machine's address and the port:
      </p>
      <CopyChip text={String(port)} mono className="text-label" />
      <p className="mt-3 text-meta text-text-muted">
        Friends outside your network need these forwarded on your router to this machine:
      </p>
      <ul className="mt-1 list-disc pl-4 text-meta text-text-secondary">
        <li>
          TCP <span className="font-mono">{port}</span>
        </li>
        <li>
          UDP{" "}
          <span className="font-mono">
            {from}–{to}
          </span>
        </li>
      </ul>
      <p className="mt-3 text-meta text-text-muted">
        Consoles (Xbox, PlayStation, Switch) can't add servers by address. Playing from your own network? Set{" "}
        <span className="font-mono">docker.bedrock_address</span> to this machine's address.
      </p>
    </Panel>
  );
}
