import { createFileRoute } from "@tanstack/react-router";
import { Server } from "lucide-react";

import { PlaceholderPage } from "@/features/placeholder-page";

export const Route = createFileRoute("/_app/servers")({
  component: () => (
    <PlaceholderPage
      title="Servers"
      eyebrow="Server fleet"
      heading="Your servers"
      description="Every world, one place. Manage your Minecraft server fleet."
      icon={<Server />}
      emptyTitle="No servers yet"
      emptyBody="Creating, starting, and stopping servers isn't available in this version yet. Your servers will appear here."
    />
  ),
});
