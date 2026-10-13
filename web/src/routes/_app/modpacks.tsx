import { createFileRoute } from "@tanstack/react-router";
import { Box } from "lucide-react";

import { PlaceholderPage } from "@/features/placeholder-page";

export const Route = createFileRoute("/_app/modpacks")({
  component: () => (
    <PlaceholderPage
      title="Modpacks"
      eyebrow="Modpack library"
      heading="Find your next adventure."
      description="Browse modpacks for your next server."
      icon={<Box />}
      emptyTitle="Modpack browsing isn't available yet"
      emptyBody="Searching Modrinth and CurseForge for modpacks arrives in a later version."
    />
  ),
});
