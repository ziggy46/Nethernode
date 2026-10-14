import { createClient } from "@connectrpc/connect";

import { AccountService } from "@/gen/nethernode/v1/account_pb";
import { transport } from "@/lib/api";

/** The signed-in user's own security RPCs. Loaded with those screens only. */
export const accountClient = createClient(AccountService, transport);
