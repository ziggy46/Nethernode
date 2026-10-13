import { Bell } from "lucide-react";

import { EmptyState } from "@/components/page";
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from "@/components/ui/dialog";

/** Recent activity. Loaded lazily by the top bar on first open. */
export default function NotificationsDialog({
  open,
  onOpenChange,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>Recent activity</DialogTitle>
          <DialogDescription>Your workspace, at a glance.</DialogDescription>
        </DialogHeader>
        <EmptyState compact icon={<Bell />} title="Nothing to show yet">
          Server, player, and backup activity will appear here once those features are available.
        </EmptyState>
      </DialogContent>
    </Dialog>
  );
}
