import { createClient } from "@connectrpc/connect";

import { ProxyService } from "@/gen/nethernode/v1/proxy_pb";
import { transport } from "@/lib/api";

/** Kept out of `@/lib/api` so the proxy's definitions load with the pages that use them, not the app shell. */
export const proxyClient = createClient(ProxyService, transport);
