import { useQuery } from "@tanstack/react-query";
import { CalendarClock, Gavel, History, Package, Play, RotateCcw, Square, TriangleAlert } from "lucide-react";

import { ago } from "@/lib/time";
import { useNow } from "@/lib/use-now";
import { cn } from "@/lib/utils";

import { auditClient } from "./api";
import { activityTitle } from "./labels";

export const activityKey = ["activity"] as const;

function icon(action: string) {
  if (action === "server.start" || action === "server.create") return <Play />;
  if (action === "server.stop" || action === "server.kill" || action === "server.delete") return <Square />;
  if (action === "server.restart") return <RotateCcw />;
  if (action.startsWith("backup.")) return <History />;
  if (action === "schedule.run") return <CalendarClock />;
  if (action.startsWith("player.")) return <Gavel />;
  return <Package />;
}

/** Recent server, backup, and player events. */
export default function ActivityFeed({ limit = 5, className }: { limit?: number; className?: string }) {
  const now = useNow();
  const activity = useQuery({
    queryKey: [...activityKey, limit],
    queryFn: () => auditClient.listActivity({ limit }),
  });
  if (activity.isError) {
    return (
      <p className="flex items-center gap-2 text-meta text-text-muted">
        <TriangleAlert className="size-3.5" aria-hidden />
        Couldn't load recent activity.
      </p>
    );
  }
  if (!activity.data) {
    return <div className="h-24 animate-pulse rounded-md bg-surface" role="status" aria-busy />;
  }
  if (activity.data.items.length === 0) {
    return (
      <p className="text-meta text-text-muted">Server starts, backups, and player bans will show up here.</p>
    );
  }
  return (
    <ul className={cn("flex flex-col gap-3.5", className)}>
      {activity.data.items.map((item) => (
        <li key={item.id} className="flex items-start gap-3">
          <span
            className="grid size-7 shrink-0 place-items-center rounded-md bg-surface-elevated text-text-secondary [&_svg]:size-3.5"
            aria-hidden
          >
            {icon(item.action)}
          </span>
          <div className="min-w-0">
            <p className="truncate text-label">{activityTitle(item.action, item.serverName)}</p>
            <p className="truncate text-meta text-text-muted">
              {[item.note, item.actor ? `by ${item.actor}` : "", ago(item.createdAtMs, now)]
                .filter(Boolean)
                .join(" · ")}
            </p>
          </div>
        </li>
      ))}
    </ul>
  );
}
