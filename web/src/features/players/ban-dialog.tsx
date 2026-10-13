import { useId, useState } from "react";
import { toast } from "sonner";

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
import { ErrorAlert, Field, Input, Select } from "@/components/ui/field";
import { errorMessage } from "@/lib/api";

import { playerClient } from "./api";

const DURATIONS: [string, number | null][] = [
  ["Permanent", null],
  ["1 hour", 3600],
  ["1 day", 86_400],
  ["7 days", 7 * 86_400],
  ["30 days", 30 * 86_400],
  ["Custom", -1],
];

export interface BanTarget {
  name: string;
  uuid?: string;
  /** Last known address, possibly masked. */
  ip?: string;
}

/** Ban one or more players, on this server or (admins) every server. */
export function BanDialog({
  open,
  onOpenChange,
  players,
  serverId,
  serverName,
  canBanEverywhere,
  onDone,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  players: BanTarget[];
  serverId?: string;
  serverName?: string;
  canBanEverywhere: boolean;
  onDone: () => void;
}) {
  const ids = { duration: useId(), ip: useId(), everywhere: useId() };
  const [reason, setReason] = useState("");
  const [preset, setPreset] = useState("0");
  const [customAmount, setCustomAmount] = useState("12");
  const [customUnit, setCustomUnit] = useState("3600");
  const [banIp, setBanIp] = useState(false);
  const [everywhere, setEverywhere] = useState(!serverId);
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string>();

  const selected = DURATIONS[Number(preset)]?.[1] ?? null;
  const seconds =
    selected === -1 ? Math.round(Number(customAmount) * Number(customUnit)) : (selected ?? undefined);
  const customInvalid = selected === -1 && (!Number.isFinite(seconds) || (seconds ?? 0) < 60);
  const single = players.length === 1 ? players[0] : undefined;

  const submit = async () => {
    setPending(true);
    setError(undefined);
    const warnings: string[] = [];
    try {
      for (const player of players) {
        const reply = await playerClient.banPlayer({
          serverId: everywhere ? undefined : serverId,
          player: { name: player.name, uuid: player.uuid },
          reason,
          durationSeconds: seconds === undefined ? undefined : BigInt(seconds),
          banIp,
        });
        warnings.push(...reply.warnings);
      }
      toast.success(single ? `Banned ${single.name}.` : `Banned ${players.length} players.`);
      for (const w of warnings) toast.warning(w, { duration: 10_000 });
      onDone();
      onOpenChange(false);
    } catch (err) {
      setError(errorMessage(err));
    } finally {
      setPending(false);
    }
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>{single ? `Ban ${single.name}` : `Ban ${players.length} players`}</DialogTitle>
          <DialogDescription>
            {everywhere
              ? "Banned players can't join any server, including servers added later."
              : `Banned players can't join ${serverName ?? "this server"}. Players online are disconnected.`}
          </DialogDescription>
        </DialogHeader>
        {error && <ErrorAlert>{error}</ErrorAlert>}
        <form
          onSubmit={(e) => {
            e.preventDefault();
            if (!customInvalid) void submit();
          }}
        >
          <Field
            label="Reason"
            placeholder="Shown to the player"
            maxLength={200}
            value={reason}
            onChange={(e) => setReason(e.target.value)}
          />
          <div className="mb-[17px]">
            <label htmlFor={ids.duration} className="mb-[7px] block text-label text-text-secondary">
              Length
            </label>
            <Select id={ids.duration} value={preset} onChange={(e) => setPreset(e.target.value)}>
              {DURATIONS.map(([label], i) => (
                <option key={label} value={i}>
                  {label}
                </option>
              ))}
            </Select>
            {selected === -1 && (
              <div className="mt-2 flex gap-2">
                <Input
                  aria-label="Custom length"
                  inputMode="decimal"
                  className="w-28"
                  value={customAmount}
                  aria-invalid={customInvalid || undefined}
                  onChange={(e) => setCustomAmount(e.target.value)}
                />
                <Select aria-label="Unit" value={customUnit} onChange={(e) => setCustomUnit(e.target.value)}>
                  <option value="3600">hours</option>
                  <option value="86400">days</option>
                </Select>
              </div>
            )}
            {customInvalid && <p className="mt-1.5 text-meta text-danger">Choose at least a minute.</p>}
            {seconds !== undefined && !customInvalid && (
              <p className="mt-1.5 text-meta text-text-muted">
                Lifted automatically on {new Date(Date.now() + seconds * 1000).toLocaleString()}.
              </p>
            )}
          </div>
          <div className="mb-[17px] flex items-start gap-2.5">
            <Checkbox
              id={ids.ip}
              checked={banIp}
              onCheckedChange={(v) => setBanIp(v === true)}
              className="mt-0.5"
            />
            <label htmlFor={ids.ip} className="text-label text-text-secondary">
              Also block their IP address
              <span className="block text-meta text-text-muted">
                {single?.ip
                  ? `Last seen from ${single.ip}. Blocked by NetherNode's proxy, so it also stops other accounts from that address.`
                  : "Uses the last address NetherNode saw them connect from."}
              </span>
            </label>
          </div>
          {serverId && canBanEverywhere && (
            <div className="mb-[17px] flex items-start gap-2.5">
              <Checkbox
                id={ids.everywhere}
                checked={everywhere}
                onCheckedChange={(v) => setEverywhere(v === true)}
                className="mt-0.5"
              />
              <label htmlFor={ids.everywhere} className="text-label text-text-secondary">
                Ban on every server
              </label>
            </div>
          )}
          <DialogFooter>
            <Button onClick={() => onOpenChange(false)}>Cancel</Button>
            <Button type="submit" variant="danger" disabled={pending || customInvalid}>
              {pending ? "Banning…" : "Ban"}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
