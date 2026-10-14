import { createClient } from "@connectrpc/connect";

import { AuditService } from "@/gen/nethernode/v1/audit_pb";
import { transport } from "@/lib/api";

export const auditClient = createClient(AuditService, transport);
