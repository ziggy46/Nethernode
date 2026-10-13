import { Square } from "lucide-react";

import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";

/** Confirmation before stopping (UI reference "Stop this server?"). */
export function StopDialog({
  open,
  onOpenChange,
  name,
  players,
  onConfirm,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  name: string;
  players: number | undefined;
  onConfirm: () => void;
}) {
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>Stop this server?</DialogTitle>
          <DialogDescription>
            {players
              ? `${players} player${players === 1 ? "" : "s"} on ${name} will be disconnected.`
              : `Players on ${name} will be disconnected.`}
          </DialogDescription>
        </DialogHeader>
        <p className="mb-4 text-label text-text-secondary">
          The world is saved before the server stops. If it doesn't stop within a minute, it's stopped
          forcefully.
        </p>
        <DialogFooter>
          <Button onClick={() => onOpenChange(false)}>Keep running</Button>
          <Button
            variant="danger"
            onClick={() => {
              onConfirm();
              onOpenChange(false);
            }}
          >
            <Square />
            Stop server
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
