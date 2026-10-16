import { useMutation, useQueryClient } from "@tanstack/react-query";
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
import { ErrorAlert, Field } from "@/components/ui/field";
import { Switch } from "@/components/ui/switch";
import { proxyClient } from "@/features/proxy/client";
import type { Listener } from "@/gen/nethernode/v1/proxy_pb";
import { errorMessage } from "@/lib/api";

export const PROXY_PROTOCOL_WARNING =
  "Every server behind this listener must expect the PROXY header (Paper: proxies.proxy-protocol: true in paper-global.yml), or players can't connect.";

/** Creates or edits a proxy listener. */
export function ListenerDialog({
  open,
  onOpenChange,
  listener,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  listener?: Listener | undefined;
}) {
  const queryClient = useQueryClient();
  const [bindAddr, setBindAddr] = useState("0.0.0.0");
  const [port, setPort] = useState("25565");
  const [proxyProtocol, setProxyProtocol] = useState(false);
  const switchId = useId();

  useEffect(() => {
    if (!open) return;
    setBindAddr(listener?.bindAddr ?? "0.0.0.0");
    setPort(String(listener?.port ?? 25565));
    setProxyProtocol(listener?.proxyProtocol ?? false);
  }, [open, listener]);

  const portNumber = Number(port);
  const portValid = Number.isInteger(portNumber) && portNumber >= 1 && portNumber <= 65535;
  const save = useMutation({
    mutationFn: () =>
      proxyClient.upsertListener({
        id: listener?.id,
        bindAddr: bindAddr.trim(),
        port: portNumber,
        enabled: listener?.enabled ?? true,
        proxyProtocol,
      }),
    onSuccess: async ({ listener: saved }) => {
      await queryClient.invalidateQueries({ queryKey: ["proxy", "listeners"] });
      if (saved?.error) {
        toast.error(`Saved, but the listener couldn't start: ${saved.error}`, {
          duration: Number.POSITIVE_INFINITY,
        });
      } else {
        toast.success(`Listener ${saved?.bindAddr}:${saved?.port} saved.`);
      }
      onOpenChange(false);
    },
  });

  const onSubmit = (e: FormEvent) => {
    e.preventDefault();
    if (portValid) save.mutate();
  };

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
          <DialogTitle>{listener ? "Edit listener" : "Add a listener"}</DialogTitle>
          <DialogDescription>
            A TCP port players connect to. Changes apply without dropping players.
          </DialogDescription>
        </DialogHeader>
        <form onSubmit={onSubmit} noValidate>
          {save.isError && <ErrorAlert>{errorMessage(save.error)}</ErrorAlert>}
          <div className="grid grid-cols-[1fr_120px] gap-[13px] below-sm:grid-cols-1">
            <Field
              label="Bind address"
              className="[&_input]:font-mono"
              hint="0.0.0.0 for all IPv4, :: for all IPv6"
              value={bindAddr}
              onChange={(e) => setBindAddr(e.target.value)}
              autoFocus
            />
            <Field
              label="Port"
              className="[&_input]:font-mono"
              inputMode="numeric"
              error={portValid ? undefined : "1–65535"}
              value={port}
              onChange={(e) => setPort(e.target.value)}
            />
          </div>
          <div className="mb-[17px] flex items-start justify-between gap-4">
            <label htmlFor={switchId} className="text-label">
              <span className="block font-[550]">Send PROXY protocol</span>
              <span className="mt-0.5 block text-meta text-text-muted">
                Lets servers see players' real IP addresses.
              </span>
            </label>
            <Switch id={switchId} checked={proxyProtocol} onCheckedChange={setProxyProtocol} />
          </div>
          {proxyProtocol && (
            <p className="mb-[17px] flex gap-2 rounded-md border border-warning/30 bg-warning-subtle px-3 py-2.5 text-meta text-warning">
              <TriangleAlert className="mt-px size-3.5 shrink-0" aria-hidden />
              {PROXY_PROTOCOL_WARNING}
            </p>
          )}
          <DialogFooter>
            <Button onClick={() => onOpenChange(false)}>Cancel</Button>
            <Button
              type="submit"
              variant="primary"
              disabled={save.isPending || !portValid || !bindAddr.trim()}
            >
              {save.isPending ? "Saving…" : "Save listener"}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
