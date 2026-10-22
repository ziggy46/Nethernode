import { useQuery } from "@tanstack/react-query";
import { CircleAlert, CircleCheck, CircleDashed, LoaderCircle } from "lucide-react";

import { cn } from "@/lib/utils";

import { type DeleteProgress, deleteProgressKey } from "./queries";

/** The steps a deletion reports, in order, with their share of the bar. */
const STEPS = [
  { id: "container", label: "Stopping it and removing its container", weight: 20 },
  { id: "modules", label: "Removing its modules", weight: 10 },
  { id: "record", label: "Removing it from NetherNode", weight: 10 },
  { id: "files", label: "Deleting its world and files", weight: 60 },
] as const;

/** How a deletion is going, from the steps its machine reports. Machines
 * that don't report steps yet (older agents) show the first step until the
 * deletion finishes. */
export function DeleteProgressView({
  serverId,
  deleteData,
  state,
}: {
  serverId: string;
  deleteData: boolean;
  /** Running, finished, or failed (the error is shown by the dialog). */
  state: "running" | "done" | "failed";
}) {
  const progress = useQuery<DeleteProgress | null>({
    queryKey: deleteProgressKey(serverId),
    queryFn: () => null,
    enabled: false,
  }).data;
  const steps = STEPS.filter((s) => deleteData || s.id !== "files");
  const reached = Math.max(
    0,
    steps.findIndex((s) => s.id === progress?.step),
  );
  const fileShare =
    progress?.step === "files" && progress.total > 0 ? Math.min(1, progress.done / progress.total) : 0;
  const totalWeight = steps.reduce((sum, s) => sum + s.weight, 0);
  const percent =
    state === "done"
      ? 100
      : Math.round(
          (steps.slice(0, reached).reduce((sum, s) => sum + s.weight, 0) +
            (steps[reached]?.weight ?? 0) * fileShare) *
            (100 / totalWeight),
        );

  return (
    <div>
      <div
        className="mb-4 h-2 overflow-hidden rounded-full bg-surface-subtle"
        role="progressbar"
        aria-label="Deletion progress"
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={percent}
      >
        <div
          className={cn(
            "h-full rounded-full transition-[width] duration-300",
            state === "failed" ? "bg-danger" : "bg-primary",
          )}
          style={{ width: `${Math.max(percent, state === "running" ? 3 : 0)}%` }}
        />
      </div>
      <ol className="space-y-2" aria-live="polite">
        {steps.map((s, i) => {
          const done = state === "done" || i < reached;
          const current = !done && i === reached;
          const icon = done ? (
            <CircleCheck className="size-4 text-success" />
          ) : current && state === "failed" ? (
            <CircleAlert className="size-4 text-danger" />
          ) : current ? (
            <LoaderCircle className="size-4 animate-spin text-primary" />
          ) : (
            <CircleDashed className="size-4 text-text-muted" />
          );
          return (
            <li key={s.id} className="flex items-start gap-2.5 text-label">
              <span className="mt-0.5 shrink-0" aria-hidden>
                {icon}
              </span>
              <span className={cn(done || current ? "text-text-primary" : "text-text-muted")}>
                {s.label}
                {current && s.id === "files" && progress?.step === "files" && progress.total > 0 && (
                  <span className="block text-meta text-text-muted">
                    {progress.done.toLocaleString()} of {progress.total.toLocaleString()} files
                  </span>
                )}
                <span className="sr-only">{done ? " (done)" : current ? " (in progress)" : ""}</span>
              </span>
            </li>
          );
        })}
      </ol>
    </div>
  );
}
