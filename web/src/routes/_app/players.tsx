import { createFileRoute } from "@tanstack/react-router";

import { PlayersPage } from "@/features/players/players-page";

export const Route = createFileRoute("/_app/players")({
  component: PlayersPage,
});
