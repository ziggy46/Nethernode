import {
  ChevronDown,
  ChevronUp,
  CircleAlert,
  CircleCheck,
  CircleSlash,
  LoaderCircle,
  Pause,
  Play,
  Upload,
  X,
} from "lucide-react";
import { useState } from "react";

import { Button } from "@/components/ui/button";
import { duration } from "@/lib/time";
import { cn } from "@/lib/utils";

import { bytes } from "./format";
import { cancel, clearEnded, dismiss, pause, resume, type UploadItem, type UploadStatus } from "./store";

const ACTIVE: UploadStatus[] = ["queued", "uploading", "paused", "finishing"];

function detail(item: UploadItem): string {
  switch (item.status) {
    case "queued":
      return "Waiting…";
    case "uploading": {
      const left = item.speed > 0 ? (item.size - item.sent) / item.speed : 0;
      const rate = item.speed > 0 ? ` · ${bytes(item.speed)}/s` : "";
      const eta = left > 0 ? ` · ${left < 60 ? "under a minute" : duration(left * 1000)} left` : "";
      return `${bytes(item.sent)} of ${bytes(item.size)}${rate}${eta}`;
    }
    case "paused":
      return `Paused at ${bytes(item.sent)} of ${bytes(item.size)}`;
    case "finishing":
      return "Checking and moving into place…";
    case "cancelled":
      return "Cancelled";
    default:
      return item.message ?? "";
  }
}

function StatusIcon({ status }: { status: UploadStatus }) {
  const cls = "size-4 shrink-0";
  switch (status) {
    case "done":
      return <CircleCheck className={cn(cls, "text-success")} aria-hidden />;
    case "failed":
      return <CircleAlert className={cn(cls, "text-danger")} aria-hidden />;
    case "skipped":
    case "cancelled":
      return <CircleSlash className={cn(cls, "text-text-muted")} aria-hidden />;
    case "finishing":
      return <LoaderCircle className={cn(cls, "animate-spin text-primary")} aria-hidden />;
    default:
      return <Upload className={cn(cls, "text-primary")} aria-hidden />;
  }
}

/** Uploads in progress, in the corner of every page. */
export default function UploadTray({ items }: { items: UploadItem[] }) {
  const [open, setOpen] = useState(true);
  const running = items.filter((i) => ACTIVE.includes(i.status));
  const total = running.reduce((n, i) => n + i.size, 0);
  const sent = running.reduce((n, i) => n + i.sent, 0);
  const percent = total > 0 ? Math.floor((sent / total) * 100) : 100;
  const title =
    running.length > 0
      ? `Uploading ${running.length} file${running.length === 1 ? "" : "s"} · ${percent}%`
      : `${items.length} upload${items.length === 1 ? "" : "s"} finished`;

  return (
    <section
      aria-label="Uploads"
      className="fixed right-4 bottom-4 z-40 w-[380px] max-w-[calc(100vw-32px)] overflow-hidden rounded-[9px] border border-border bg-surface-elevated shadow-overlay"
    >
      <header className="flex items-center gap-2 border-divider border-b px-3.5 py-2.5">
        <p className="min-w-0 flex-1 truncate font-medium text-label" aria-live="polite">
          {title}
        </p>
        {running.length < items.length && (
          <Button size="mini" variant="ghost" onClick={clearEnded}>
            Clear finished
          </Button>
        )}
        <Button
          size="icon"
          variant="ghost"
          className="size-7"
          aria-expanded={open}
          aria-label={open ? "Collapse uploads" : "Expand uploads"}
          onClick={() => setOpen(!open)}
        >
          {open ? <ChevronDown /> : <ChevronUp />}
        </Button>
      </header>
      {open && (
        <ul className="max-h-[min(50vh,360px)] overflow-y-auto">
          {items.map((item) => (
            <Row key={item.key} item={item} />
          ))}
        </ul>
      )}
    </section>
  );
}

function Row({ item }: { item: UploadItem }) {
  const fraction = item.size > 0 ? item.sent / item.size : 1;
  const showBar = ["uploading", "paused", "queued", "finishing"].includes(item.status);
  const where = `${item.serverName}${item.dir ? ` / ${item.dir}` : ""}`;
  return (
    <li className="border-divider border-b px-3.5 py-2.5 last:border-b-0">
      <div className="flex items-center gap-2.5">
        <StatusIcon status={item.status} />
        <div className="min-w-0 flex-1">
          <p className="truncate text-label" title={item.name}>
            {item.name}
          </p>
          <p className="truncate text-meta text-text-muted" title={where}>
            {where}
          </p>
        </div>
        <div className="flex shrink-0 items-center">
          {item.status === "uploading" && (
            <Button
              size="icon"
              variant="ghost"
              className="size-7"
              aria-label={`Pause ${item.name}`}
              onClick={() => pause(item.key)}
            >
              <Pause />
            </Button>
          )}
          {(item.status === "paused" || (item.status === "failed" && item.sent < item.size)) && (
            <Button
              size="icon"
              variant="ghost"
              className="size-7"
              aria-label={`Resume ${item.name}`}
              onClick={() => resume(item.key)}
            >
              <Play />
            </Button>
          )}
          {["queued", "uploading", "paused"].includes(item.status) ||
          (item.status === "failed" && item.sent < item.size) ? (
            <Button
              size="icon"
              variant="ghost"
              className="size-7"
              aria-label={`Cancel ${item.name}`}
              onClick={() => cancel(item.key)}
            >
              <X />
            </Button>
          ) : (
            item.status !== "finishing" && (
              <Button
                size="icon"
                variant="ghost"
                className="size-7"
                aria-label={`Dismiss ${item.name}`}
                onClick={() => dismiss(item.key)}
              >
                <X />
              </Button>
            )
          )}
        </div>
      </div>
      {showBar && (
        <div
          className="mt-2 h-1.5 overflow-hidden rounded-full bg-surface-subtle"
          role="progressbar"
          aria-label={`${item.name} upload progress`}
          aria-valuemin={0}
          aria-valuemax={100}
          aria-valuenow={Math.floor(fraction * 100)}
        >
          <div
            className={cn(
              "h-full rounded-full transition-[width] duration-200",
              item.status === "paused" ? "bg-text-muted" : "bg-primary",
            )}
            style={{ width: `${fraction * 100}%` }}
          />
        </div>
      )}
      <p
        className={cn(
          "mt-1.5 text-meta tabular-nums",
          item.status === "failed" ? "text-danger" : "text-text-muted",
        )}
      >
        {detail(item)}
      </p>
    </li>
  );
}
