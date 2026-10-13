import { createClient } from "@connectrpc/connect";

import { FileService } from "@/gen/nethernode/v1/files_pb";
import { transport } from "@/lib/api";

/** File manager RPCs. Kept out of the shell: only the Files tab needs them. */
export const fileClient = createClient(FileService, transport);
