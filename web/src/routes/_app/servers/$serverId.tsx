import { createFileRoute } from "@tanstack/react-router";

import { ServerPage } from "@/features/servers/server-page";

export const Route = createFileRoute("/_app/servers/$serverId")({
  component: function ServerRoute() {
    const { serverId } = Route.useParams();
    return <ServerPage id={serverId} />;
  },
});
