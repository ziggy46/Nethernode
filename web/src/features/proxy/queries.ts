import { queryOptions } from "@tanstack/react-query";

import { proxyClient } from "@/features/proxy/client";

export { serversQuery } from "@/features/servers/queries";

export const listenersQuery = queryOptions({
  queryKey: ["proxy", "listeners"],
  queryFn: async () => (await proxyClient.listListeners({})).listeners,
});

export const routesQuery = queryOptions({
  queryKey: ["proxy", "routes"],
  queryFn: async () => (await proxyClient.listRoutes({})).routes,
});

export const connectionsQuery = queryOptions({
  queryKey: ["proxy", "connections"],
  queryFn: async () => (await proxyClient.listConnections({})).connections,
});
