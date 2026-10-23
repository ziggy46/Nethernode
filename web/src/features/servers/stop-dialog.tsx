import { RotateCcw, Square } from "lucide-react";
import { useEffect, useState } from "react";

import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { cn } from "@/lib/utils";

/** How long to warn players before acting, in minutes (0: now). */
const WHEN = [
  { minutes: 0, label: "Now" },
  { minutes: 1, label: "In 1 minute" },
  { minutes: 5, label: "In 5 minutes" },
];

/** Confirmation before stopping or restarting (UI reference "Stop this
 * server?"). With players online it can warn them in chat first. */
export function StopDialog({
  open,
  onOpenChange,
  name,
  players,
  restart,
  onConfirm,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  name: string;
  players: number | undefined;
  /** A restart rather than a stop. */
  restart?: boolean;
  onConfirm: (warnMinutes: number) => void;
}) {
  const online = players ?? 0;
  // With people playing, warning them first is the default.
  const [minutes, setMinutes] = useState(online > 0 ? 1 : 0);
  useEffect(() => {
    if (open) setMinutes(online > 0 ? 1 : 0);
  }, [open, online]);
  const verb = restart ? "restart" : "stop";

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>{restart ? "Restart this server?" : "Stop this server?"}</DialogTitle>
          <DialogDescription>
            {online > 0
              ? `${online} player${online === 1 ? "" : "s"} on ${name} will be disconnected.`
              : `Players on ${name} will be disconnected.`}
          </DialogDescription>
        </DialogHeader>
        {online > 0 && (
          <fieldset className="mb-4">
            <legend className="mb-[7px] text-label text-text-secondary">When</legend>
            <div className="flex flex-wrap gap-2">
              {WHEN.map((w) => (
                <label
                  key={w.minutes}
                  className={cn(
                    "cursor-pointer rounded-md border px-3 py-1.5 text-label transition-colors has-[:focus-visible]:outline-2 has-[:focus-visible]:outline-primary",
                    minutes === w.minutes
                      ? "border-primary bg-primary-subtle text-text-primary"
                      : "border-border text-text-secondary hover:bg-surface-subtle",
                  )}
                >
                  <input
                    type="radio"
                    name="stop-when"
                    value={w.minutes}
                    checked={minutes === w.minutes}
                    onChange={() => setMinutes(w.minutes)}
                    className="sr-only"
                  />
                  {w.label}
                </label>
              ))}
            </div>
            {minutes > 0 && (
              <p className="mt-2 text-meta text-text-muted">
                Players get a countdown in chat, and you can call it off until it ends.
              </p>
            )}
          </fieldset>
        )}
        <p className="mb-4 text-label text-text-secondary">
          The world is saved before the server stops. If it doesn't stop within a minute, it's stopped
          forcefully.
        </p>
        <DialogFooter>
          <Button onClick={() => onOpenChange(false)}>Keep running</Button>
          <Button
            variant={restart ? "primary" : "danger"}
            onClick={() => {
              onConfirm(minutes);
              onOpenChange(false);
            }}
          >
            {restart ? <RotateCcw /> : <Square />}
            {minutes > 0 ? `Warn and ${verb}` : restart ? "Restart server" : "Stop server"}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
