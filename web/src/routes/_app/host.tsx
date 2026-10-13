import { createFileRoute } from "@tanstack/react-router";
import { Cpu } from "lucide-react";

import { PlaceholderPage } from "@/features/placeholder-page";

export const Route = createFileRoute("/_app/host")({
  component: () => (
    <PlaceholderPage
      title="Host node"
      eyebrow="Host node"
      heading="A healthy home for your worlds."
      description="Monitor the machine behind your Minecraft servers."
      icon={<Cpu />}
      emptyTitle="Host monitoring isn't available yet"
      emptyBody="CPU, memory, storage, and Docker details for this machine will appear here."
    />
  ),
});
