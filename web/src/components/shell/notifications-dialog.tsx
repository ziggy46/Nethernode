import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import ActivityFeed from "@/features/audit/activity-feed";

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
          <DialogDescription>What has been happening on your servers.</DialogDescription>
        </DialogHeader>
        <ActivityFeed limit={12} />
      </DialogContent>
    </Dialog>
  );
}
