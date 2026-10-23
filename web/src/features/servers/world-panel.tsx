import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Globe2, Scissors } from "lucide-react";
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
import { type Server, ServerStatus } from "@/gen/nethernode/v1/servers_pb";
import { errorMessage, serverClient } from "@/lib/api";
import { jobResultKey, trackJob } from "@/lib/jobs";

import { formatBytes } from "./format";

const NAMES: Record<string, string> = {
  "minecraft:overworld": "Overworld",
  "minecraft:the_nether": "The Nether",
  "minecraft:the_end": "The End",
};

/** A dimension's name: Minecraft's own, or a mod's `namespace:name`. */
function dimensionName(id: string): string {
  return NAMES[id] ?? id;
}

/** The world's dimensions and trimming the chunks nobody spent time in. */
export function WorldPanel({ server }: { server: Server }) {
  const world = useQuery({
    queryKey: ["world", server.id],
    queryFn: () => serverClient.getWorld({ id: server.id }),
    staleTime: 5 * 60_000,
  });
  const [trimming, setTrimming] = useState(false);
  const dims = world.data?.dimensions ?? [];
  const total = dims.reduce((sum, d) => sum + Number(d.bytes), 0);

  return (
    <Panel>
      <PanelHeader title="World" icon={<Globe2 />} />
      {world.isError ? (
        <ErrorAlert>{errorMessage(world.error)}</ErrorAlert>
      ) : world.isPending ? (
        <div className="h-24 animate-pulse rounded-md bg-surface-subtle" aria-busy />
      ) : dims.length === 0 ? (
        <p className="text-label text-text-muted">
          No world yet: it's made the first time the server starts.
        </p>
      ) : (
        <>
          <table className="mb-4 w-full text-label">
            <thead>
              <tr className="text-left text-meta text-text-muted">
                <th className="pb-1.5 font-normal">Dimension</th>
                <th className="pb-1.5 text-right font-normal">Chunks</th>
                <th className="pb-1.5 text-right font-normal">Size</th>
              </tr>
            </thead>
            <tbody className="divide-y divide-divider">
              {dims.map((d) => (
                <tr key={d.id}>
                  <td className="py-1.5">{dimensionName(d.id)}</td>
                  <td className="py-1.5 text-right tabular-nums">{Number(d.chunks).toLocaleString()}</td>
                  <td className="py-1.5 text-right tabular-nums">{formatBytes(Number(d.bytes))}</td>
                </tr>
              ))}
              {dims.length > 1 && (
                <tr className="font-[550]">
                  <td className="py-1.5">Total</td>
                  <td />
                  <td className="py-1.5 text-right tabular-nums">{formatBytes(total)}</td>
                </tr>
              )}
            </tbody>
          </table>
          <Button size="mini" onClick={() => setTrimming(true)}>
            <Scissors />
            Trim unvisited chunks…
          </Button>
        </>
      )}
      <Pregenerate server={server} />
      {trimming && <TrimDialog server={server} onClose={() => setTrimming(false)} />}
    </Panel>
  );
}

/** Pre-generating around the spawn with Chunky. */
function Pregenerate({ server }: { server: Server }) {
  const id = useId();
  const queryClient = useQueryClient();
  const [radius, setRadius] = useState(2000);
  const [reply, setReply] = useState<string | null>(null);
  const status = useQuery({
    queryKey: ["pregen", server.id, server.status],
    queryFn: () => serverClient.getPregen({ id: server.id }),
    // Follows a running task.
    refetchInterval: (q) => (q.state.data?.running ? 5000 : false),
  });
  const refresh = () => queryClient.invalidateQueries({ queryKey: ["pregen", server.id] });
  const install = useMutation({
    mutationFn: () => serverClient.installChunky({ id: server.id }),
    onSuccess: ({ message }) => {
      setReply(message);
      void refresh();
    },
  });
  const start = useMutation({
    mutationFn: (confirm: boolean) => serverClient.startPregen({ id: server.id, radius, confirm }),
    onSuccess: ({ message }) => {
      setReply(message);
      void refresh();
    },
  });
  const cancel = useMutation({
    mutationFn: () => serverClient.cancelPregen({ id: server.id }),
    onSuccess: ({ message }) => {
      setReply(message);
      void refresh();
    },
  });
  const s = status.data;
  if (!s) return null;
  const error = install.error ?? start.error ?? cancel.error;
  // Chunky asks before redoing a task or when disk space looks short.
  const asks = reply?.includes("type '") ?? false;

  return (
    <div className="mt-5 border-divider border-t pt-4">
      <p className="mb-1 font-[550] text-label">Pre-generate</p>
      <p className="mb-3 text-meta text-text-muted">
        Generates the world around the spawn ahead of time with Chunky, so exploring there doesn't lag.
      </p>
      {error && <ErrorAlert>{errorMessage(error)}</ErrorAlert>}
      {s.unavailable ? (
        <p className="text-label text-text-muted">{s.unavailable}</p>
      ) : !s.installed ? (
        <Button size="mini" disabled={install.isPending} onClick={() => install.mutate()}>
          {install.isPending ? "Installing…" : "Install Chunky"}
        </Button>
      ) : !s.loaded ? (
        <p className="text-label text-text-muted">
          {s.serverRunning
            ? "Chunky is installed; restart the server to load it."
            : "Chunky is installed. Start the server to pre-generate."}
        </p>
      ) : s.running ? (
        <div>
          <div
            className="mb-2 h-2 overflow-hidden rounded-full bg-surface-subtle"
            role="progressbar"
            aria-label="Pre-generation progress"
            aria-valuemin={0}
            aria-valuemax={100}
            aria-valuenow={Math.round(s.percent)}
          >
            <div
              className="h-full rounded-full bg-primary"
              style={{ width: `${Math.min(100, s.percent)}%` }}
            />
          </div>
          <p className="mb-3 text-meta text-text-muted">
            {s.percent.toFixed(1)}% · {Number(s.chunks).toLocaleString()} chunks
            {s.eta ? ` · about ${s.eta} left` : ""}
          </p>
          <Button size="mini" disabled={cancel.isPending} onClick={() => cancel.mutate()}>
            Stop pre-generating
          </Button>
        </div>
      ) : (
        <div className="flex flex-wrap items-center gap-2">
          <label htmlFor={id} className="text-label text-text-secondary">
            Radius
          </label>
          <Select
            id={id}
            className="h-8 w-auto"
            value={radius}
            onChange={(e) => setRadius(Number(e.target.value))}
          >
            {[1000, 2000, 5000, 10000].map((r) => (
              <option key={r} value={r}>
                {r.toLocaleString()} blocks
              </option>
            ))}
          </Select>
          <Button
            size="mini"
            variant="primary"
            disabled={start.isPending}
            onClick={() => start.mutate(false)}
          >
            Start
          </Button>
        </div>
      )}
      {reply && (
        <p className="mt-3 text-meta text-text-secondary" aria-live="polite">
          {reply}
          {asks && (
            <Button
              size="mini"
              className="ml-2"
              disabled={start.isPending}
              onClick={() => start.mutate(true)}
            >
              Go ahead
            </Button>
          )}
        </p>
      )}
    </div>
  );
}

function TrimDialog({ server, onClose }: { server: Server; onClose: () => void }) {
  const ids = { minutes: useId(), radius: useId() };
  const [minutes, setMinutes] = useState(2);
  const [radius, setRadius] = useState(16);
  const [previewJob, setPreviewJob] = useState<string | null>(null);
  const stopped = server.status === ServerStatus.STOPPED || server.status === ServerStatus.CRASHED;
  const preview = useQuery<{ message: string; failed: boolean } | null>({
    queryKey: jobResultKey(previewJob ?? ""),
    queryFn: () => null,
    enabled: false,
  }).data;
  const check = useMutation({
    mutationFn: () => serverClient.trimWorld({ id: server.id, minutes, keepRadius: radius, apply: false }),
    onSuccess: ({ jobId }) => setPreviewJob(jobId),
  });
  const trim = useMutation({
    mutationFn: () => serverClient.trimWorld({ id: server.id, minutes, keepRadius: radius, apply: true }),
    onSuccess: ({ jobId }) => {
      trackJob(jobId, `Backing up and trimming ${server.name}'s world…`);
      onClose();
    },
  });
  const counting = previewJob !== null && !preview;
  // A new choice needs a new count.
  const change = (fn: () => void) => {
    fn();
    setPreviewJob(null);
  };

  return (
    <Dialog open onOpenChange={(open) => !open && onClose()}>
      <DialogContent className="w-[560px]">
        <DialogHeader>
          <DialogTitle>Trim unvisited chunks</DialogTitle>
          <DialogDescription>
            Removes chunks players spent almost no time in, so the world takes less space. They generate
            again, fresh, when someone gets there; builds and changes in them are lost.
          </DialogDescription>
        </DialogHeader>
        <div className="mb-4 grid grid-cols-2 gap-[13px] below-sm:grid-cols-1">
          <div>
            <label htmlFor={ids.minutes} className="mb-[7px] block text-label text-text-secondary">
              Players spent less than
            </label>
            <Select
              id={ids.minutes}
              value={minutes}
              onChange={(e) => change(() => setMinutes(Number(e.target.value)))}
            >
              {[1, 2, 5, 15, 60].map((m) => (
                <option key={m} value={m}>
                  {m === 60 ? "1 hour" : `${m} minute${m === 1 ? "" : "s"}`}
                </option>
              ))}
            </Select>
          </div>
          <div>
            <label htmlFor={ids.radius} className="mb-[7px] block text-label text-text-secondary">
              Always keep around spawn
            </label>
            <Select
              id={ids.radius}
              value={radius}
              onChange={(e) => change(() => setRadius(Number(e.target.value)))}
            >
              {[8, 16, 32, 64].map((r) => (
                <option key={r} value={r}>
                  {r} chunks ({(r * 16).toLocaleString()} blocks)
                </option>
              ))}
            </Select>
          </div>
        </div>
        <p className="mb-4 text-meta text-text-muted">
          The End's central island always stays. Chunks that can't be read are kept.
        </p>
        {(check.error || trim.error) && <ErrorAlert>{errorMessage(check.error ?? trim.error)}</ErrorAlert>}
        <div className="mb-4 rounded-md border border-divider px-3 py-2.5 text-label" aria-live="polite">
          {preview ? (
            <span className={preview.failed ? "text-danger" : ""}>{preview.message}</span>
          ) : counting ? (
            <span className="text-text-muted">Counting… (big worlds take a while)</span>
          ) : (
            <span className="text-text-muted">Check first to see how much would go.</span>
          )}
        </div>
        {!stopped && (
          <p className="mb-4 text-meta text-warning">Stop the server to trim; you can check while it runs.</p>
        )}
        <DialogFooter>
          <Button onClick={onClose}>Cancel</Button>
          <Button disabled={check.isPending || counting} onClick={() => check.mutate()}>
            {counting ? "Checking…" : "Check"}
          </Button>
          <Button
            variant="danger"
            disabled={!stopped || !preview || preview.failed || trim.isPending}
            onClick={() => trim.mutate()}
          >
            <Scissors />
            Back up and trim
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
