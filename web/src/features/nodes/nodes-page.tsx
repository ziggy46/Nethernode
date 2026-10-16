import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { Copy, Cpu, KeyRound, Pencil, Plus, RefreshCw, Trash2, TriangleAlert } from "lucide-react";
import { useId, useState } from "react";
import { toast } from "sonner";

import { ConfirmDialog } from "@/components/confirm-dialog";
import { EmptyState, Footer, PageHeader, Panel } from "@/components/page";
import { Badge, Dot } from "@/components/status-badge";
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
import { formatBytes } from "@/features/servers/format";
import { type JoinCode, type Node, NodeStatus } from "@/gen/nethernode/v1/nodes_pb";
import { errorMessage } from "@/lib/api";
import { healthQuery } from "@/lib/health";
import { useCanManage } from "@/lib/session";
import { ago } from "@/lib/time";
import { useDocumentTitle } from "@/lib/use-document-title";
import { useNow } from "@/lib/use-now";

import { joinCommand, nodeClient, nodesKey, nodesQuery } from "./queries";

type Dialogs =
  | { kind: "add" }
  | { kind: "join"; node: Node; join: JoinCode }
  | { kind: "rename"; node: Node }
  | { kind: "remove"; node: Node };

/** The machines that run servers. */
export function NodesPage() {
  useDocumentTitle("Nodes");
  const health = useQuery(healthQuery);
  const nodes = useQuery(nodesQuery);
  const canManage = useCanManage();
  const [dialog, setDialog] = useState<Dialogs | null>(null);
  const unavailable = nodes.data?.unavailableReason;

  return (
    <>
      <PageHeader
        eyebrow="Nodes"
        title="Every machine, one panel."
        description="The machines that run your servers. Players still connect through this panel."
        actions={
          canManage && (
            <Button
              variant="primary"
              disabled={Boolean(unavailable)}
              onClick={() => setDialog({ kind: "add" })}
            >
              <Plus />
              Add a node
            </Button>
          )
        }
      />
      {unavailable && <ErrorAlert>Other machines can't join: {unavailable}.</ErrorAlert>}
      {canManage && nodes.data?.nodes.some((n) => !n.local) && (
        <AutoUpdate enabled={nodes.data.autoUpdate} version={nodes.data.panelVersion} />
      )}
      {nodes.isError ? (
        <div className="rounded-lg border border-divider border-dashed bg-surface-subtle">
          <EmptyState
            icon={<TriangleAlert />}
            title="Couldn't load nodes"
            action={<Button onClick={() => void nodes.refetch()}>Try again</Button>}
          >
            {errorMessage(nodes.error)}
          </EmptyState>
        </div>
      ) : !nodes.data ? (
        <div className="h-48 animate-pulse rounded-lg bg-surface-subtle" role="status" aria-busy>
          <span className="sr-only">Loading nodes…</span>
        </div>
      ) : (
        <div className="grid grid-cols-2 gap-[18px] below-lg:grid-cols-1">
          {nodes.data.nodes.map((node) => (
            <NodeCard
              key={node.id}
              node={node}
              panelVersion={nodes.data.panelVersion}
              canManage={canManage}
              onAction={setDialog}
            />
          ))}
        </div>
      )}
      {dialog?.kind === "add" && (
        <AddDialog
          onClose={() => setDialog(null)}
          onCreated={(node, join) => setDialog({ kind: "join", node, join })}
        />
      )}
      {dialog?.kind === "join" && (
        <JoinDialog node={dialog.node} join={dialog.join} onClose={() => setDialog(null)} />
      )}
      {dialog?.kind === "rename" && <RenameDialog node={dialog.node} onClose={() => setDialog(null)} />}
      {dialog?.kind === "remove" && <RemoveDialog node={dialog.node} onClose={() => setDialog(null)} />}
      <Footer version={health.data?.version} />
    </>
  );
}

/** The switch for updating older nodes as they connect. */
function AutoUpdate({ enabled, version }: { enabled: boolean; version: string }) {
  const client = useQueryClient();
  const id = useId();
  const set = useMutation({
    mutationFn: (on: boolean) => nodeClient.setAutoUpdate({ enabled: on }),
    onSuccess: () => void client.invalidateQueries({ queryKey: nodesKey }),
    onError: (e) => toast.error(errorMessage(e)),
  });
  return (
    <div className="mb-[18px] flex items-start justify-between gap-5 rounded-lg border border-divider bg-surface-subtle px-4 py-3">
      <label htmlFor={id} className="text-label">
        <span className="block font-[550]">Update nodes automatically</span>
        <span className="mt-0.5 block text-meta text-text-muted">
          Nodes on an older version than this panel (v{version}) update themselves when they connect. Their
          servers keep running.
        </span>
      </label>
      <Switch
        id={id}
        checked={set.isPending ? Boolean(set.variables) : enabled}
        disabled={set.isPending}
        onCheckedChange={(on) => set.mutate(on)}
      />
    </div>
  );
}

function StatusBadge({ node }: { node: Node }) {
  if (node.updating) {
    return (
      <Badge tone="info" icon={<Dot pulse />}>
        Updating
      </Badge>
    );
  }
  if (node.status === NodeStatus.ONLINE && node.updateNeeded) {
    return (
      <Badge tone="warning" icon={<Dot />}>
        Update needed
      </Badge>
    );
  }
  switch (node.status) {
    case NodeStatus.ONLINE:
      return (
        <Badge tone="success" icon={<Dot />}>
          Online
        </Badge>
      );
    case NodeStatus.WAITING:
      return (
        <Badge tone="info" icon={<Dot pulse />}>
          Waiting to join
        </Badge>
      );
    default:
      return (
        <Badge tone="danger" icon={<Dot />}>
          Offline
        </Badge>
      );
  }
}

function NodeCard({
  node,
  panelVersion,
  canManage,
  onAction,
}: {
  node: Node;
  panelVersion: string;
  canManage: boolean;
  onAction: (dialog: Dialogs) => void;
}) {
  const now = useNow();
  const client = useQueryClient();
  const update = useMutation({
    mutationFn: () => nodeClient.updateNode({ id: node.id }),
    onSuccess: () => void client.invalidateQueries({ queryKey: nodesKey }),
    onError: (e) => toast.error(errorMessage(e)),
  });
  const regenerate = useMutation({
    mutationFn: () => nodeClient.regenerateJoinCode({ id: node.id }),
    onSuccess: ({ join }) => join && onAction({ kind: "join", node, join }),
    onError: (e) => toast.error(errorMessage(e)),
  });
  const host = node.host;
  const facts: [string, string][] = host
    ? [
        ["Machine", [host.hostname, host.operatingSystem].filter(Boolean).join(" · ") || "—"],
        [
          "Docker",
          host.dockerAvailable
            ? `Docker ${host.dockerVersion}`
            : `Unavailable${host.dockerError ? `: ${host.dockerError}` : ""}`,
        ],
        ["CPU and memory", host.cpus ? `${host.cpus} cores · ${formatBytes(Number(host.memoryBytes))}` : "—"],
        [
          "Disk",
          host.diskTotalBytes !== undefined && host.diskFreeBytes !== undefined
            ? `${formatBytes(Number(host.diskFreeBytes))} free of ${formatBytes(Number(host.diskTotalBytes))}`
            : "—",
        ],
      ]
    : [];
  if (node.version) {
    facts.push([
      "NetherNode",
      node.updateAvailable ? `v${node.version} (panel: v${panelVersion})` : `v${node.version}`,
    ]);
  }
  if (node.status === NodeStatus.OFFLINE && node.lastSeenMs !== undefined) {
    facts.push(["Last heard from", ago(node.lastSeenMs, now)]);
  }

  return (
    <Panel className="flex flex-col">
      <div className="mb-3 flex items-start justify-between gap-3">
        <div className="min-w-0">
          <h2 className="truncate font-semibold text-section">{node.name}</h2>
          <p className="mt-[3px] text-meta text-text-muted">
            {node.local ? "The machine this panel runs on" : "An agent on another machine"} ·{" "}
            {node.serverCount === 1 ? "1 server" : `${node.serverCount} servers`}
          </p>
        </div>
        <StatusBadge node={node} />
      </div>
      {node.status === NodeStatus.WAITING ? (
        <p className="text-meta text-text-secondary">
          {node.joinPending
            ? "Run the join command on that machine. It connects as soon as the agent starts."
            : "The join code expired. Make a new one and run the command on that machine."}
        </p>
      ) : (
        <dl className="text-meta">
          {facts.map(([label, value]) => (
            <div
              key={label}
              className="flex justify-between gap-3 border-divider border-b py-[9px] last:border-b-0"
            >
              <dt className="shrink-0 text-text-muted">{label}</dt>
              <dd className="min-w-0 truncate text-right" title={value}>
                {value}
              </dd>
            </div>
          ))}
        </dl>
      )}
      {node.updateError && !node.updating && (
        <p className="mt-3 text-danger text-meta" role="alert">
          The update didn't work: {node.updateError}
        </p>
      )}
      <div className="mt-auto flex flex-wrap gap-2 pt-4">
        {canManage && node.status === NodeStatus.ONLINE && node.updateAvailable && (
          <Button
            size="mini"
            variant="primary"
            disabled={node.updating || update.isPending}
            onClick={() => update.mutate()}
          >
            <RefreshCw />
            {node.updating ? "Updating…" : `Update to v${panelVersion}`}
          </Button>
        )}
        {node.local && (
          <Button size="mini" asChild>
            <Link to="/host">
              <Cpu />
              Details
            </Link>
          </Button>
        )}
        {canManage && node.status === NodeStatus.WAITING && (
          <Button size="mini" disabled={regenerate.isPending} onClick={() => regenerate.mutate()}>
            <KeyRound />
            {node.joinPending ? "New join code" : "Make a join code"}
          </Button>
        )}
        {canManage && (
          <Button size="mini" onClick={() => onAction({ kind: "rename", node })}>
            <Pencil />
            Rename
          </Button>
        )}
        {canManage && !node.local && (
          <Button size="mini" variant="danger" onClick={() => onAction({ kind: "remove", node })}>
            <Trash2 />
            Remove
          </Button>
        )}
      </div>
    </Panel>
  );
}

function nameError(name: string): string | undefined {
  const trimmed = name.trim();
  if (!trimmed) return "Give the node a name.";
  if ([...trimmed].length > 40) return "Keep the name to 40 characters.";
  return undefined;
}

function AddDialog({
  onClose,
  onCreated,
}: {
  onClose: () => void;
  onCreated: (node: Node, join: JoinCode) => void;
}) {
  const client = useQueryClient();
  const [name, setName] = useState("");
  const [touched, setTouched] = useState(false);
  const create = useMutation({
    mutationFn: () => nodeClient.createNode({ name: name.trim() }),
    onSuccess: ({ node, join }) => {
      void client.invalidateQueries({ queryKey: nodesKey });
      if (node && join) onCreated(node, join);
    },
  });
  const error = touched ? nameError(name) : undefined;
  return (
    <Dialog open onOpenChange={(o) => !o && onClose()}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>Add a node</DialogTitle>
          <DialogDescription>
            Another machine with Docker that runs servers for this panel. Next you'll get a command to run on
            it.
          </DialogDescription>
        </DialogHeader>
        <form
          onSubmit={(e) => {
            e.preventDefault();
            setTouched(true);
            if (!nameError(name)) create.mutate();
          }}
          noValidate
        >
          {create.isError && <ErrorAlert>{errorMessage(create.error)}</ErrorAlert>}
          <Field
            label="Name"
            placeholder="Basement PC"
            autoComplete="off"
            autoFocus
            error={error}
            value={name}
            onChange={(e) => setName(e.target.value)}
          />
          <DialogFooter>
            <Button onClick={onClose}>Cancel</Button>
            <Button type="submit" variant="primary" disabled={create.isPending}>
              <Plus />
              {create.isPending ? "Adding…" : "Add node"}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}

function JoinDialog({ node, join, onClose }: { node: Node; join: JoinCode; onClose: () => void }) {
  const nodes = useQuery(nodesQuery);
  const now = useNow();
  const live = nodes.data?.nodes.find((n) => n.id === node.id);
  const joined = live !== undefined && live.status !== NodeStatus.WAITING;
  const command = joinCommand(join, window.location.origin);
  const minutes = Math.max(0, Math.ceil((Number(join.expiresAtMs) - now) / 60_000));
  const copy = async () => {
    try {
      await navigator.clipboard.writeText(command);
      toast.success("Command copied.");
    } catch {
      toast.error("Couldn't copy. Select the command and copy it instead.");
    }
  };
  return (
    <Dialog open onOpenChange={(o) => !o && onClose()}>
      <DialogContent className="w-[680px]">
        <DialogHeader>
          <DialogTitle>Join {node.name} to this panel</DialogTitle>
          <DialogDescription>
            Run this on that machine. It needs Docker, and it connects out to this panel, so nothing needs
            opening on it. The code works once, for{" "}
            {minutes === 1 ? "1 more minute" : `${minutes} more minutes`}.
          </DialogDescription>
        </DialogHeader>
        <pre className="select-all overflow-x-auto whitespace-pre rounded-md border border-divider bg-console p-3.5 font-mono text-console-text text-meta">
          {command}
        </pre>
        <p className="mt-3 flex items-center gap-2 text-label" role="status">
          {joined ? (
            <Badge tone="success" icon={<Dot />}>
              Connected
            </Badge>
          ) : (
            <Badge tone="info" icon={<Dot pulse />}>
              Waiting for the agent…
            </Badge>
          )}
        </p>
        <DialogFooter>
          <Button onClick={() => void copy()}>
            <Copy />
            Copy
          </Button>
          <Button variant="primary" onClick={onClose}>
            {joined ? "Done" : "Close"}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

function RenameDialog({ node, onClose }: { node: Node; onClose: () => void }) {
  const client = useQueryClient();
  const [name, setName] = useState(node.name);
  const rename = useMutation({
    mutationFn: () => nodeClient.renameNode({ id: node.id, name: name.trim() }),
    onSuccess: () => {
      void client.invalidateQueries({ queryKey: nodesKey });
      onClose();
    },
  });
  const error = nameError(name);
  return (
    <Dialog open onOpenChange={(o) => !o && onClose()}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>Rename {node.name}</DialogTitle>
          <DialogDescription>Only the name in this panel changes.</DialogDescription>
        </DialogHeader>
        <form
          onSubmit={(e) => {
            e.preventDefault();
            if (!error) rename.mutate();
          }}
          noValidate
        >
          {rename.isError && <ErrorAlert>{errorMessage(rename.error)}</ErrorAlert>}
          <Field
            label="Name"
            autoComplete="off"
            autoFocus
            error={error}
            value={name}
            onChange={(e) => setName(e.target.value)}
          />
          <DialogFooter>
            <Button onClick={onClose}>Cancel</Button>
            <Button type="submit" variant="primary" disabled={rename.isPending || name.trim() === node.name}>
              {rename.isPending ? "Saving…" : "Rename"}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}

function RemoveDialog({ node, onClose }: { node: Node; onClose: () => void }) {
  const client = useQueryClient();
  const remove = useMutation({
    mutationFn: () => nodeClient.deleteNode({ id: node.id, forgetServers: node.serverCount > 0 }),
    onSuccess: () => {
      void client.invalidateQueries({ queryKey: nodesKey });
      toast.success(`${node.name} was removed.`);
      onClose();
    },
  });
  return (
    <ConfirmDialog
      open
      onOpenChange={(o) => !o && onClose()}
      title={`Remove ${node.name}?`}
      description={
        node.serverCount > 0
          ? `Its agent is disconnected and can't come back without a new join code. The panel forgets its ${node.serverCount === 1 ? "server" : `${node.serverCount} servers`} and the backups kept there, stopping ${node.serverCount === 1 ? "it" : "them"} first if the machine is online. Their files stay on that machine. To keep a server, move it first.`
          : "Its agent is disconnected and can't come back without a new join code. Nothing on that machine is deleted."
      }
      confirmLabel={node.serverCount > 0 ? "Remove and forget servers" : "Remove node"}
      pending={remove.isPending}
      error={remove.error ? errorMessage(remove.error) : undefined}
      onConfirm={() => remove.mutate()}
    />
  );
}
