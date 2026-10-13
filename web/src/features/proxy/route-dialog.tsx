import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { TriangleAlert } from "lucide-react";
import { type FormEvent, useEffect, useId, useState } from "react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";

import { ErrorAlert, Field, Select } from "@/components/ui/field";
import type { Route } from "@/gen/nethernode/v1/proxy_pb";
import { errorMessage, proxyClient } from "@/lib/api";

import { listenersQuery, serversQuery } from "./queries";

const SLUG_WILDCARD = "__slug__";

/** IPv4 literal, or anything with a colon (IPv6). */
export function looksLikeIpAddress(hostname: string): boolean {
  const h = hostname.trim().replace(/^\[|\]$/g, "");
  return /^\d{1,3}(\.\d{1,3}){3}$/.test(h) || (h.includes(":") && /^[0-9a-f:.]+$/i.test(h));
}

/** Creates or edits a hostname route. */
export function RouteDialog({
  open,
  onOpenChange,
  route,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  route?: Route | undefined;
}) {
  const queryClient = useQueryClient();
  const servers = useQuery({ ...serversQuery, enabled: open });
  const listeners = useQuery({ ...listenersQuery, enabled: open });
  const [hostname, setHostname] = useState("");
  const [serverId, setServerId] = useState("");
  const [listenerId, setListenerId] = useState("");
  const destinationId = useId();
  const listenerFieldId = useId();

  useEffect(() => {
    if (!open) return;
    setHostname(route?.hostname ?? "");
    setServerId(route ? (route.serverId ?? SLUG_WILDCARD) : "");
    setListenerId(route?.listenerId ?? "");
  }, [open, route]);

  const isWildcard = hostname.trim().startsWith("*.");
  const save = useMutation({
    mutationFn: () =>
      proxyClient.upsertRoute({
        id: route?.id,
        hostname: hostname.trim(),
        serverId: serverId && serverId !== SLUG_WILDCARD ? serverId : undefined,
        listenerId: listenerId || undefined,
      }),
    onSuccess: async ({ route: saved }) => {
      // Server cards list the hostnames that reach each server.
      await Promise.all([
        queryClient.invalidateQueries({ queryKey: ["proxy", "routes"] }),
        queryClient.invalidateQueries({ queryKey: ["servers"] }),
      ]);
      toast.success(route ? `Route ${saved?.hostname} updated.` : `Route ${saved?.hostname} added.`);
      onOpenChange(false);
    },
  });

  const onSubmit = (e: FormEvent) => {
    e.preventDefault();
    save.mutate();
  };

  const noServers = servers.isSuccess && servers.data.length === 0;
  return (
    <Dialog
      open={open}
      onOpenChange={(next) => {
        if (!next) save.reset();
        onOpenChange(next);
      }}
    >
      <DialogContent>
        <DialogHeader>
          <DialogTitle>{route ? "Edit route" : "Add a server route"}</DialogTitle>
          <DialogDescription>
            Send players who connect with a hostname to one of your servers.
          </DialogDescription>
        </DialogHeader>
        <form onSubmit={onSubmit} noValidate>
          {save.isError && <ErrorAlert>{errorMessage(save.error)}</ErrorAlert>}
          <Field
            label="Hostname"
            name="hostname"
            placeholder="play.example.com"
            autoComplete="off"
            autoCapitalize="none"
            spellCheck={false}
            className="[&_input]:font-mono"
            hint="An exact name, *.example.com for any subdomain, or * for everything else."
            value={hostname}
            onChange={(e) => setHostname(e.target.value)}
            autoFocus
          />
          {looksLikeIpAddress(hostname) && (
            <p className="mb-[17px] flex gap-2 rounded-md border border-warning/30 bg-warning-subtle px-3 py-2.5 text-meta text-warning">
              <TriangleAlert className="mt-px size-3.5 shrink-0" aria-hidden />
              Players who join by IP address may not send that address: some clients send a reverse-DNS name
              instead (e.g. localhost), even though the server list looks fine. Use a hostname, or * to catch
              everyone joining by IP.
            </p>
          )}
          <div className="mb-[17px]">
            <label htmlFor={destinationId} className="mb-[7px] block text-label text-text-secondary">
              Destination
            </label>
            <Select
              id={destinationId}
              value={serverId}
              onChange={(e) => setServerId(e.target.value)}
              disabled={servers.isPending}
            >
              <option value="" disabled>
                {servers.isPending ? "Loading servers…" : "Choose a server"}
              </option>
              {isWildcard && <option value={SLUG_WILDCARD}>Server whose slug matches the subdomain</option>}
              {servers.data?.map((s) => (
                <option key={s.id} value={s.id}>
                  {s.name} ({s.slug})
                </option>
              ))}
            </Select>
            {noServers && !isWildcard && (
              <p className="mt-1.5 text-meta text-text-muted">
                No servers yet. Routes need a destination, and creating servers isn't available in this
                version yet.
              </p>
            )}
            {servers.isError && <p className="mt-1.5 text-meta text-danger">{errorMessage(servers.error)}</p>}
          </div>
          <div className="mb-[17px]">
            <label htmlFor={listenerFieldId} className="mb-[7px] block text-label text-text-secondary">
              Listener
            </label>
            <Select id={listenerFieldId} value={listenerId} onChange={(e) => setListenerId(e.target.value)}>
              <option value="">All listeners</option>
              {listeners.data?.map((l) => (
                <option key={l.id} value={l.id}>
                  {l.bindAddr}:{l.port}
                </option>
              ))}
            </Select>
          </div>
          <DialogFooter>
            <Button onClick={() => onOpenChange(false)}>Cancel</Button>
            <Button
              type="submit"
              variant="primary"
              disabled={save.isPending || !hostname.trim() || !serverId}
            >
              {save.isPending ? "Saving…" : "Save route"}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
