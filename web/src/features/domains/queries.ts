import { queryOptions, useMutation, useQueryClient } from "@tanstack/react-query";

import { proxyClient } from "@/lib/api";

export const domainsKey = ["proxy", "domains"] as const;

/** The admin's domains; the first is the default for new servers. */
export const domainsQuery = queryOptions({
  queryKey: domainsKey,
  queryFn: async () => (await proxyClient.listDomains({})).domains,
});

/** Replaces the domain list; the server cleans it up and checks it. */
export function useSetDomains() {
  const client = useQueryClient();
  return useMutation({
    mutationFn: async (domains: string[]) => (await proxyClient.setDomains({ domains })).domains,
    onSuccess: (domains) => client.setQueryData(domainsKey, domains),
  });
}

/** `example.com` from what someone typed, or why not (mirrors the server's rules). */
export function domainError(input: string): string | undefined {
  const host = input.trim().toLowerCase().replace(/^\*\./, "").replace(/\.$/, "");
  if (!host) return "Enter a domain.";
  if (/^\d+(\.\d+){3}$/.test(host) || host.includes(":")) return "Use a domain name, not an IP address.";
  const label = /^[a-z0-9_]([a-z0-9_-]{0,61}[a-z0-9_])?$/;
  if (!host.includes(".") || host.length > 253 || !host.split(".").every((l) => label.test(l))) {
    return "Enter a domain like example.com or mc.example.com.";
  }
  return undefined;
}
