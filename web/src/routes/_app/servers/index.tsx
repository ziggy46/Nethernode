import { createFileRoute } from "@tanstack/react-router";

import { ServersPage } from "@/features/servers/servers-page";

export const Route = createFileRoute("/_app/servers/")({ component: ServersPage });
