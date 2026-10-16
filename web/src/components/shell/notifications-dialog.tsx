import { Link, useNavigate } from "@tanstack/react-router";
import { CircleAlert, Info, TriangleAlert } from "lucide-react";
import { useEffect } from "react";

import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { markNoticesSeen, type Notice } from "@/lib/notices";
import { cn } from "@/lib/utils";

const ICONS = {
  danger: <CircleAlert className="size-4 text-danger" aria-hidden />,
  warning: <TriangleAlert className="size-4 text-warning" aria-hidden />,
  info: <Info className="size-4 text-info" aria-hidden />,
};

/** What needs attention. Loaded lazily by the top bar on first open. */
export default function NotificationsDialog({
  open,
  onOpenChange,
  notices,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  notices: Notice[] | undefined;
}) {
  useEffect(() => {
    if (open && notices) markNoticesSeen(notices);
  }, [open, notices]);

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>Notifications</DialogTitle>
          <DialogDescription>
            Things that need your attention: problems with servers, machines and backups, and new versions of
            NetherNode.
          </DialogDescription>
        </DialogHeader>
        {notices === undefined ? (
          <div className="h-24 animate-pulse rounded-md bg-surface-subtle" aria-busy />
        ) : notices.length === 0 ? (
          <p className="rounded-md border border-divider border-dashed px-4 py-6 text-center text-label text-text-muted">
            Nothing needs your attention.{" "}
            <Link to="/audit" className="text-primary hover:underline" onClick={() => onOpenChange(false)}>
              See what's been happening
            </Link>{" "}
            in the audit log.
          </p>
        ) : (
          <ul className="flex flex-col gap-2">
            {notices.map((notice) => (
              <li
                key={notice.id}
                className="flex gap-3 rounded-md border border-divider bg-surface-subtle px-3.5 py-3"
              >
                <span className="mt-0.5 shrink-0">{ICONS[notice.level]}</span>
                <div className="min-w-0 text-label">
                  <p className="font-[550]">{notice.title}</p>
                  <p className="mt-0.5 text-meta text-text-muted">{notice.detail}</p>
                  {notice.link && <NoticeLink link={notice.link} onGo={() => onOpenChange(false)} />}
                </div>
              </li>
            ))}
          </ul>
        )}
      </DialogContent>
    </Dialog>
  );
}

function NoticeLink({ link, onGo }: { link: string; onGo: () => void }) {
  const navigate = useNavigate();
  const className = cn("mt-1.5 inline-block text-meta text-primary hover:underline");
  if (/^https?:\/\//.test(link)) {
    return (
      <a href={link} target="_blank" rel="noreferrer" className={className}>
        Read the release notes
      </a>
    );
  }
  return (
    <a
      href={link}
      className={className}
      onClick={(e) => {
        e.preventDefault();
        onGo();
        void navigate({ to: link as never });
      }}
    >
      Open
    </a>
  );
}
