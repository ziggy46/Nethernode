import { createClient } from "@connectrpc/connect";

import { CatalogService } from "@/gen/nethernode/v1/catalog_pb";
import { ModService } from "@/gen/nethernode/v1/mods_pb";
import { transport } from "@/lib/api";

export const catalogClient = createClient(CatalogService, transport);
export const modClient = createClient(ModService, transport);
