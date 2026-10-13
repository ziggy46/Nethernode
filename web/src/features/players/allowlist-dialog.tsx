import { Download, UserPlus } from "lucide-react";
import { useId, useState } from "react";
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
import { ErrorAlert, Input } from "@/components/ui/field";
import { Switch } from "@/components/ui/switch";
import type { PlayerRow } from "@/gen/nethernode/v1/players_pb";
import { errorMessage } from "@/lib/api";

import { playerClient } from "./api";

/** Allowlist enforcement, adding by username, import, and export. */
export function AllowlistDialog({
  open,
  onOpenChange,
  serverId,
  serverName,
  enabled,
  players,
  onChanged,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  serverId: string;
  serverName: string;
  enabled: boolean;
  players: PlayerRow[];
  onChanged: () => void;
}) {
  const ids = { add: useId(), toggle: useId(), import: useId() };
  const [name, setName] = useState("");
  const [bulk, setBulk] = useState("");
  const [pending, setPending] = useState<string | null>(null);
  const [error, setError] = useState<string>();
  const listed = players.filter((p) => p.allowlisted);

  const run = async (what: string, work: () => Promise<void>) => {
    setPending(what);
    setError(undefined);
    try {
      await work();
      onChanged();
    } catch (err) {
      setError(errorMessage(err));
    } finally {
      setPending(null);
    }
  };

  const add = () =>
    run("add", async () => {
      const resolved = await playerClient.resolvePlayer({ serverId, name: name.trim() });
      await playerClient.setAllowlisted({
        serverId,
        player: { name: resolved.name, uuid: resolved.uuid },
        allowed: true,
      });
      toast.success(`Added ${resolved.name}${resolved.offline ? " (offline-mode UUID)" : ""}.`);
      setName("");
    });

  const importNames = () =>
    run("import", async () => {
      const names = bulk.split(/[\s,;]+/).filter(Boolean);
      const reply = await playerClient.importAllowlist({ serverId, names });
      if (reply.added.length)
        toast.success(`Added ${reply.added.length} player${reply.added.length === 1 ? "" : "s"}.`);
      if (reply.failed.length) setError(`Couldn't add: ${reply.failed.join("; ")}`);
      else setBulk("");
    });

  const exportList = () => {
    const data = JSON.stringify(
      listed.map((p) => ({ uuid: p.uuid, name: p.name })),
      null,
      2,
    );
    const url = URL.createObjectURL(new Blob([data], { type: "application/json" }));
    const a = document.createElement("a");
    a.href = url;
    a.download = `${serverName.replace(/[^\w.-]+/g, "-")}-allowlist.json`;
    a.click();
    URL.revokeObjectURL(url);
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="w-[520px]">
        <DialogHeader>
          <DialogTitle>Allowlist</DialogTitle>
          <DialogDescription>When it's on, only players on the list can join {serverName}.</DialogDescription>
        </DialogHeader>
        {error && <ErrorAlert>{error}</ErrorAlert>}
        <div className="mb-[17px] flex items-center justify-between gap-4 rounded-md border border-divider px-3 py-2.5">
          <label htmlFor={ids.toggle} className="text-label">
            Only allow listed players
            <span className="block text-meta text-text-muted">{listed.length} on the list</span>
          </label>
          <Switch
            id={ids.toggle}
            checked={enabled}
            disabled={pending !== null}
            onCheckedChange={(on) =>
              run("toggle", async () => {
                await playerClient.setAllowlistEnabled({ serverId, enabled: on });
                toast.success(on ? "Allowlist turned on." : "Allowlist turned off.");
              })
            }
          />
        </div>
        <form
          className="mb-[17px]"
          onSubmit={(e) => {
            e.preventDefault();
            if (name.trim()) void add();
          }}
        >
          <label htmlFor={ids.add} className="mb-[7px] block text-label text-text-secondary">
            Add a player
          </label>
          <div className="flex gap-2">
            <Input
              id={ids.add}
              placeholder="Username"
              autoComplete="off"
              spellCheck={false}
              value={name}
              onChange={(e) => setName(e.target.value)}
            />
            <Button type="submit" disabled={!name.trim() || pending !== null}>
              <UserPlus />
              {pending === "add" ? "Adding…" : "Add"}
            </Button>
          </div>
        </form>
        <div className="mb-[17px]">
          <label htmlFor={ids.import} className="mb-[7px] block text-label text-text-secondary">
            Import usernames
          </label>
          <textarea
            id={ids.import}
            rows={4}
            placeholder="One per line, or separated by commas"
            value={bulk}
            onChange={(e) => setBulk(e.target.value)}
            className="block w-full rounded-md border border-border-strong bg-sidebar px-3 py-2 font-mono text-label text-text-primary placeholder:text-text-muted focus-visible:border-primary focus-visible:outline-2 focus-visible:outline-primary focus-visible:outline-offset-2"
          />
          <Button
            className="mt-2"
            size="mini"
            disabled={!bulk.trim() || pending !== null}
            onClick={importNames}
          >
            {pending === "import" ? "Importing…" : "Import"}
          </Button>
        </div>
        <DialogFooter>
          <Button className="mr-auto" onClick={exportList} disabled={listed.length === 0}>
            <Download />
            Export
          </Button>
          <Button onClick={() => onOpenChange(false)}>Done</Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
