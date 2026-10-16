import { useMutation, useQuery } from "@tanstack/react-query";
import { ArrowRightLeft, Server as ServerIcon } from "lucide-react";
import { useId, useState } from "react";

import { Panel, PanelHeader } from "@/components/page";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { ErrorAlert, Select } from "@/components/ui/field";
import { NodeStatus } from "@/gen/nethernode/v1/nodes_pb";
import { type Server, ServerStatus } from "@/gen/nethernode/v1/servers_pb";
import { errorMessage, serverClient } from "@/lib/api";
import { trackJob } from "@/lib/jobs";

import { nodesQuery } from "../nodes/queries";

const MOVABLE = [ServerStatus.STOPPED, ServerStatus.CRASHED, ServerStatus.SLEEPING];

/** Which machine runs the server, and moving it to another. Shown once there's more than one machine. */
export function MoveServer({ server }: { server: Server }) {
  const { data } = useQuery(nodesQuery);
  const [open, setOpen] = useState(false);
  const [target, setTarget] = useState("");
  const selectId = useId();
  const others = (data?.nodes ?? []).filter((n) => n.id !== server.nodeId);
  const usable = others.filter((n) => n.status === NodeStatus.ONLINE && !n.updateNeeded);
  const chosen = usable.find((n) => n.id === target) ?? usable[0];
  const here = server.nodeName || "this machine";

  const move = useMutation({
    mutationFn: (to: string) => serverClient.moveServer({ id: server.id, nodeId: to }),
    onSuccess: ({ jobId }) => {
      setOpen(false);
      trackJob(jobId, `Moving ${server.name} to ${chosen?.name ?? "another machine"}…`);
    },
  });

  if (others.length === 0) return null;
  const stopped = MOVABLE.includes(server.status);

  return (
    <Panel>
      <PanelHeader title="Machine" icon={<ServerIcon />} />
      <div className="flex items-center justify-between gap-5 below-sm:flex-col below-sm:items-start">
        <div className="text-label">
          <p className="font-[550]">Runs on {here}</p>
          <p className="mt-0.5 text-meta text-text-muted">
            {stopped
              ? "Move it to another machine. Its world, files and settings go with it."
              : "Stop the server to move it to another machine."}
          </p>
        </div>
        <Button
          onClick={() => setOpen(true)}
          disabled={!stopped || usable.length === 0 || server.nodeOffline}
        >
          <ArrowRightLeft />
          Move…
        </Button>
      </div>
      <Dialog
        open={open}
        onOpenChange={(next) => {
          setOpen(next);
          if (!next) move.reset();
        }}
      >
        <DialogContent>
          <DialogHeader>
            <DialogTitle>Move {server.name}</DialogTitle>
            <DialogDescription>
              The server is archived on {here}, copied to the new machine and set up there, then removed from{" "}
              {here}. The archive stays on {here} as a backup. Players can't join until the move is done.
            </DialogDescription>
          </DialogHeader>
          {move.isError && <ErrorAlert>{errorMessage(move.error)}</ErrorAlert>}
          <label htmlFor={selectId} className="mb-1.5 block text-label font-[550]">
            Move to
          </label>
          <Select id={selectId} value={chosen?.id ?? ""} onChange={(e) => setTarget(e.target.value)}>
            {others.map((n) => (
              <option key={n.id} value={n.id} disabled={n.status !== NodeStatus.ONLINE || n.updateNeeded}>
                {n.name}
                {n.status !== NodeStatus.ONLINE ? " (offline)" : n.updateNeeded ? " (needs an update)" : ""}
              </option>
            ))}
          </Select>
          <DialogFooter>
            <Button onClick={() => setOpen(false)}>Cancel</Button>
            <Button
              variant="primary"
              disabled={!chosen || move.isPending}
              onClick={() => chosen && move.mutate(chosen.id)}
            >
              <ArrowRightLeft />
              {move.isPending ? "Starting…" : `Move to ${chosen?.name ?? "…"}`}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </Panel>
  );
}
