import { createClient } from "@connectrpc/connect";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { HardDrive, Trash2 } from "lucide-react";
import { useState } from "react";
import { toast } from "sonner";

import { ConfirmDialog } from "@/components/confirm-dialog";
import { Panel, PanelHeader } from "@/components/page";
import { Button } from "@/components/ui/button";
import { ErrorAlert } from "@/components/ui/field";
import { CatalogService } from "@/gen/nethernode/v1/catalog_pb";
import { errorMessage, transport } from "@/lib/api";

const client = createClient(CatalogService, transport);
const key = ["cache-usage"] as const;

const KINDS: Record<string, { label: string; note: string }> = {
  minecraft: {
    label: "Minecraft game files",
    note: "Used to draw items in player inventories, about 30 MB per Minecraft version.",
  },
  mods: { label: "Copies of mod files", note: "Used to draw items that come from mods." },
  icons: { label: "Mod and modpack icons", note: "Shown when browsing and in mod lists." },
  heads: { label: "Player faces", note: "Shown next to player names." },
};

function size(bytes: number): string {
  if (bytes >= 1024 ** 3) return `${(bytes / 1024 ** 3).toFixed(1)} GB`;
  if (bytes >= 1024 ** 2) return `${(bytes / 1024 ** 2).toFixed(bytes >= 10 * 1024 ** 2 ? 0 : 1)} MB`;
  return `${Math.ceil(bytes / 1024)} KB`;
}

/** What the panel keeps of its downloads, and clearing it. Admins only. */
export default function CachePanel() {
  const queryClient = useQueryClient();
  const usage = useQuery({ queryKey: key, queryFn: () => client.getCacheUsage({}) });
  const [confirm, setConfirm] = useState(false);
  const clear = useMutation({
    mutationFn: () => client.clearCache({}),
    onSuccess: (r) => {
      setConfirm(false);
      toast.success(`Cleared ${size(Number(r.freedBytes))} of downloaded files.`);
      void queryClient.invalidateQueries({ queryKey: key });
    },
  });
  const parts = usage.data?.parts ?? [];
  const total = parts.reduce((sum, p) => sum + Number(p.bytes), 0);

  return (
    <Panel>
      <PanelHeader title="Downloaded files" icon={<HardDrive />} />
      <p className="mb-4 text-meta text-text-muted">
        The panel keeps copies of what it downloads so pages load quickly. Anything unused for two weeks is
        removed automatically (game files stay while a server uses that Minecraft version), and everything
        here is downloaded again when it's needed.
      </p>
      {usage.isError ? (
        <ErrorAlert>{errorMessage(usage.error)}</ErrorAlert>
      ) : !usage.data ? (
        <div className="h-32 animate-pulse rounded-md bg-surface" role="status" aria-busy />
      ) : (
        <>
          <ul className="divide-y divide-divider border-divider border-y">
            {parts.map((p) => {
              const kind = KINDS[p.kind] ?? { label: p.kind, note: "" };
              return (
                <li key={p.kind} className="flex items-baseline justify-between gap-4 py-2.5">
                  <div className="min-w-0">
                    <p className="font-[550] text-label">{kind.label}</p>
                    <p className="text-meta text-text-muted">{kind.note}</p>
                  </div>
                  <p className="shrink-0 text-label tabular-nums">
                    {p.files === 0n ? "Empty" : size(Number(p.bytes))}
                  </p>
                </li>
              );
            })}
          </ul>
          <div className="mt-4 flex flex-wrap items-center justify-between gap-3">
            <p className="text-label">
              <span className="font-[550]">{size(total)}</span>
              <span className="text-text-muted"> in total</span>
            </p>
            <Button size="mini" disabled={total === 0} onClick={() => setConfirm(true)}>
              <Trash2 />
              Clear now
            </Button>
          </div>
        </>
      )}
      <ConfirmDialog
        open={confirm}
        onOpenChange={(open) => {
          setConfirm(open);
          if (!open) clear.reset();
        }}
        title="Clear downloaded files?"
        description="Item pictures, icons and player faces load more slowly the next time they're shown, while the panel downloads them again. Servers aren't affected."
        confirmLabel={clear.isPending ? "Clearing…" : "Clear now"}
        pending={clear.isPending}
        error={clear.error ? errorMessage(clear.error) : undefined}
        onConfirm={() => clear.mutate()}
      />
    </Panel>
  );
}
