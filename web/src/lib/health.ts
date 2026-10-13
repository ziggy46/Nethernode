import { queryOptions } from "@tanstack/react-query";

export interface Health {
  status: string;
  version: string;
}

/** `/healthz`: whether the panel API is reachable, and its version. */
export const healthQuery = queryOptions({
  queryKey: ["health"],
  queryFn: async (): Promise<Health> => {
    const res = await fetch("/healthz", { credentials: "same-origin" });
    if (!res.ok) throw new Error(`health check failed: ${res.status}`);
    return (await res.json()) as Health;
  },
  refetchInterval: 30_000,
  retry: false,
});
