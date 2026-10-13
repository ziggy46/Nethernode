import { createFileRoute } from "@tanstack/react-router";
import { Network } from "lucide-react";

import { PlaceholderPage } from "@/features/placeholder-page";

export const Route = createFileRoute("/_app/proxy")({
  component: () => (
    <PlaceholderPage
      title="Proxy & routes"
      eyebrow="Smart routing"
      heading="One address. Every world."
      description="Route players to the right server with a hostname."
      icon={<Network />}
      emptyTitle="No routes yet"
      emptyBody="Hostname routing isn't available in this version yet. Routes and live proxy connections will appear here."
    />
  ),
});
