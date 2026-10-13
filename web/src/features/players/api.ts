import { createClient } from "@connectrpc/connect";

import { PlayerService } from "@/gen/nethernode/v1/players_pb";
import { transport } from "@/lib/api";

/** Player RPCs. Loaded with the players pages only. */
export const playerClient = createClient(PlayerService, transport);
