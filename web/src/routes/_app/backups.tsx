import { createFileRoute } from "@tanstack/react-router";
import { History } from "lucide-react";

import { PlaceholderPage } from "@/features/placeholder-page";

export const Route = createFileRoute("/_app/backups")({
  component: () => (
    <PlaceholderPage
      title="Backups"
      eyebrow="World protection"
      heading="Keep every world safe."
      description="Automatic snapshots, so you can focus on the adventure."
      icon={<History />}
      emptyTitle="No backups yet"
      emptyBody="Backups and schedules aren't available in this version yet. Snapshots will be listed here."
    />
  ),
});
