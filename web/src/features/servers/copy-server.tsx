import { useMutation, useQuery } from "@tanstack/react-query";
import { Copy, LayoutTemplate } from "lucide-react";
import { useId, useState } from "react";

import { Panel, PanelHeader } from "@/components/page";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { ErrorAlert, Field, Select } from "@/components/ui/field";
import { NodeStatus } from "@/gen/nethernode/v1/nodes_pb";
import { type Server, ServerStatus } from "@/gen/nethernode/v1/servers_pb";
import { errorMessage, serverClient } from "@/lib/api";
import { trackJob } from "@/lib/jobs";

import { nodesQuery } from "../nodes/queries";

/** Machines a new server can go on, and the choice among them. */
export function useMachines(open: boolean, preferred: string) {
  const nodes = (useQuery({ ...nodesQuery, enabled: open }).data?.nodes ?? []).filter(
    (n) => n.status !== NodeStatus.WAITING,
  );
  const [picked, setPicked] = useState("");
  const usable = (id: string) =>
    nodes.some((n) => n.id === id && n.status === NodeStatus.ONLINE && !n.updateNeeded);
  // With no machine list (yet), the panel's own.
  const chosen =
    nodes.length === 0
      ? "local"
      : usable(picked)
        ? picked
        : usable(preferred)
          ? preferred
          : (nodes.find((n) => usable(n.id))?.id ?? "");
  return { nodes, chosen, pick: setPicked, name: nodes.find((n) => n.id === chosen)?.name ?? "" };
}

export function MachineSelect({ machines }: { machines: ReturnType<typeof useMachines> }) {
  const id = useId();
  if (machines.nodes.length < 2) return null;
  return (
    <div className="mb-[17px]">
      <label htmlFor={id} className="mb-[7px] block text-label text-text-secondary">
        Machine
      </label>
      <Select id={id} value={machines.chosen} onChange={(e) => machines.pick(e.target.value)}>
        {machines.nodes.map((n) => (
          <option key={n.id} value={n.id} disabled={n.status !== NodeStatus.ONLINE || n.updateNeeded}>
            {n.name}
            {n.status !== NodeStatus.ONLINE ? " (offline)" : n.updateNeeded ? " (needs an update)" : ""}
          </option>
        ))}
      </Select>
    </div>
  );
}

/** Making a copy of the server, and saving it as a template. */
export function CopyServer({ server }: { server: Server }) {
  const [dialog, setDialog] = useState<"copy" | "template" | null>(null);
  const busy = [ServerStatus.INSTALLING, ServerStatus.STARTING, ServerStatus.STOPPING].includes(
    server.status,
  );
  return (
    <Panel>
      <PanelHeader title="Copies and templates" icon={<Copy />} />
      <div className="flex items-center justify-between gap-5 below-sm:flex-col below-sm:items-start">
        <p className="text-meta text-text-muted">
          Make a separate copy of this server, with or without its world, or save its setup (software, mods,
          plugins and configuration, without the world) as a template for new servers.
        </p>
        <div className="flex shrink-0 gap-2">
          <Button disabled={busy || server.nodeOffline} onClick={() => setDialog("copy")}>
            <Copy />
            Make a copy…
          </Button>
          <Button disabled={busy || server.nodeOffline} onClick={() => setDialog("template")}>
            <LayoutTemplate />
            Save as template…
          </Button>
        </div>
      </div>
      <CopyDialog
        server={server}
        open={dialog === "copy"}
        onOpenChange={(o) => setDialog(o ? "copy" : null)}
      />
      <TemplateDialog
        server={server}
        open={dialog === "template"}
        onOpenChange={(o) => setDialog(o ? "template" : null)}
      />
    </Panel>
  );
}

function CopyDialog({
  server,
  open,
  onOpenChange,
}: {
  server: Server;
  open: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  const [name, setName] = useState("");
  const [world, setWorld] = useState(true);
  const worldId = useId();
  const machines = useMachines(open, server.nodeId);
  const copyName = name.trim() || `${server.name} copy`;
  const copy = useMutation({
    mutationFn: () =>
      serverClient.copyServer({
        id: server.id,
        name: copyName,
        nodeId: machines.chosen === "local" ? "" : machines.chosen,
        includeWorld: world,
      }),
    onSuccess: ({ jobId }) => {
      onOpenChange(false);
      trackJob(jobId, `Copying ${server.name}…`);
    },
  });
  const running = server.status === ServerStatus.RUNNING;
  return (
    <Dialog
      open={open}
      onOpenChange={(o) => {
        if (!o) copy.reset();
        onOpenChange(o);
      }}
    >
      <DialogContent>
        <DialogHeader>
          <DialogTitle>Copy {server.name}</DialogTitle>
          <DialogDescription>
            The copy is a separate server with its own address and ports. It starts out stopped. Schedules,
            routes and backups stay with {server.name}.
            {running && " The world is saved first; players stay connected."}
          </DialogDescription>
        </DialogHeader>
        {copy.isError && <ErrorAlert>{errorMessage(copy.error)}</ErrorAlert>}
        <form
          onSubmit={(e) => {
            e.preventDefault();
            copy.mutate();
          }}
        >
          <Field
            label="Name"
            placeholder={`${server.name} copy`}
            value={name}
            maxLength={64}
            onChange={(e) => setName(e.target.value)}
          />
          <MachineSelect machines={machines} />
          <div className="flex items-start gap-2.5">
            <Checkbox id={worldId} checked={world} onCheckedChange={(v) => setWorld(v === true)} />
            <label htmlFor={worldId} className="text-label">
              <span className="block font-[550]">Copy the world too</span>
              <span className="mt-0.5 block text-meta text-text-muted">
                Otherwise the copy makes a new world when it first starts.
              </span>
            </label>
          </div>
          <DialogFooter>
            <Button onClick={() => onOpenChange(false)}>Cancel</Button>
            <Button type="submit" variant="primary" disabled={!machines.chosen || copy.isPending}>
              <Copy />
              {copy.isPending ? "Starting…" : "Make the copy"}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}

function TemplateDialog({
  server,
  open,
  onOpenChange,
}: {
  server: Server;
  open: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  const [name, setName] = useState("");
  const templateName = name.trim() || server.name;
  const save = useMutation({
    mutationFn: () => serverClient.saveServerTemplate({ id: server.id, name: templateName }),
    onSuccess: ({ jobId }) => {
      onOpenChange(false);
      trackJob(jobId, `Saving ${server.name} as a template…`);
    },
  });
  return (
    <Dialog
      open={open}
      onOpenChange={(o) => {
        if (!o) save.reset();
        onOpenChange(o);
      }}
    >
      <DialogContent>
        <DialogHeader>
          <DialogTitle>Save {server.name} as a template</DialogTitle>
          <DialogDescription>
            Keeps a copy of its software, mods, plugins, configuration and settings, without the world. New
            servers can start from it: it appears when you click Create server.
          </DialogDescription>
        </DialogHeader>
        {save.isError && <ErrorAlert>{errorMessage(save.error)}</ErrorAlert>}
        <form
          onSubmit={(e) => {
            e.preventDefault();
            save.mutate();
          }}
        >
          <Field
            label="Template name"
            placeholder={server.name}
            value={name}
            maxLength={64}
            onChange={(e) => setName(e.target.value)}
          />
          <DialogFooter>
            <Button onClick={() => onOpenChange(false)}>Cancel</Button>
            <Button type="submit" variant="primary" disabled={save.isPending}>
              <LayoutTemplate />
              {save.isPending ? "Starting…" : "Save template"}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
