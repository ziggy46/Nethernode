import { createFileRoute } from "@tanstack/react-router";

import { ModpacksPage } from "@/features/modpacks/modpacks-page";

export const Route = createFileRoute("/_app/modpacks")({
  component: ModpacksPage,
});
