import { CircleAlert, Moon } from "lucide-react";

import { ServerStatus } from "@/gen/nethernode/v1/servers_pb";
import { cn } from "@/lib/utils";

type Tone = "success" | "warning" | "info" | "muted" | "danger";

const TONES: Record<Tone, string> = {
  success: "border-success/25 bg-success-subtle text-success",
  warning: "border-warning/25 bg-warning-subtle text-warning",
  info: "border-info/25 bg-info-subtle text-info",
  danger: "border-danger/25 bg-danger-subtle text-danger",
  muted: "border-border bg-surface-elevated text-text-secondary",
};

/** A compact status chip: color always paired with an icon and a label. */
export function Badge({
  tone,
  icon,
  children,
  className,
}: {
  tone: Tone;
  icon?: React.ReactNode;
  children: React.ReactNode;
  className?: string;
}) {
  return (
    <span
      className={cn(
        "inline-flex shrink-0 items-center gap-1.5 whitespace-nowrap rounded-sm border px-[7px] py-[3px] text-meta leading-none",
        TONES[tone],
        className,
      )}
    >
      {icon}
      {children}
    </span>
  );
}

/** A filled dot for running/live states. */
export function Dot({ className, pulse = false }: { className?: string; pulse?: boolean }) {
  return (
    <span
      aria-hidden
      className={cn("inline-block size-1.5 rounded-full bg-current", pulse && "animate-pulse", className)}
    />
  );
}

const SERVER_STATUS: Record<ServerStatus, { tone: Tone; label: string }> = {
  [ServerStatus.UNSPECIFIED]: { tone: "muted", label: "Unknown" },
  [ServerStatus.INSTALLING]: { tone: "warning", label: "Installing…" },
  [ServerStatus.STOPPED]: { tone: "muted", label: "Stopped" },
  [ServerStatus.STARTING]: { tone: "warning", label: "Starting…" },
  [ServerStatus.RUNNING]: { tone: "success", label: "Running" },
  [ServerStatus.STOPPING]: { tone: "warning", label: "Stopping…" },
  [ServerStatus.SLEEPING]: { tone: "info", label: "Sleeping" },
  [ServerStatus.CRASHED]: { tone: "danger", label: "Crashed" },
};

/**
 * Server lifecycle badge, per 06-design-system §3.4. A server whose machine
 * is offline says so instead: its last known status may be stale.
 */
export function ServerStatusBadge({ status, nodeOffline }: { status: ServerStatus; nodeOffline?: boolean }) {
  if (nodeOffline) {
    return (
      <Badge tone="muted" icon={<CircleAlert className="size-3" aria-hidden />}>
        Machine offline
      </Badge>
    );
  }
  const { tone, label } = SERVER_STATUS[status];
  const icon =
    status === ServerStatus.SLEEPING ? (
      <Moon className="size-3" aria-hidden />
    ) : status === ServerStatus.CRASHED ? (
      <CircleAlert className="size-3" aria-hidden />
    ) : status === ServerStatus.STOPPED || status === ServerStatus.UNSPECIFIED ? (
      <span aria-hidden className="inline-block size-1.5 rounded-full border border-current" />
    ) : (
      <Dot pulse={tone === "warning"} />
    );
  return (
    <Badge tone={tone} icon={icon}>
      {label}
    </Badge>
  );
}
