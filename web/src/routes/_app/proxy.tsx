import { createFileRoute } from "@tanstack/react-router";

import { ProxyPage } from "@/features/proxy/proxy-page";

export const Route = createFileRoute("/_app/proxy")({ component: ProxyPage });
