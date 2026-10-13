import { createFileRoute } from "@tanstack/react-router";

import { HostPage } from "@/features/host/host-page";

export const Route = createFileRoute("/_app/host")({ component: HostPage });
