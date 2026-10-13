import { cn } from "@/lib/utils";

type Tone = "cpu" | "memory" | "storage";

const FILL: Record<Tone, string> = {
  cpu: "bg-meter-cpu",
  memory: "bg-meter-memory",
  storage: "bg-meter-storage",
};

/**
 * A thin meter with its value as text, so it never relies on color alone.
 * `percent` is clamped to 0–100; `null` shows an empty track.
 */
export function ResourceMeter({
  label,
  value,
  percent,
  tone,
  size = "sm",
}: {
  label: string;
  value: React.ReactNode;
  percent: number | null;
  tone: Tone;
  size?: "sm" | "md";
}) {
  const clamped = percent === null ? 0 : Math.min(100, Math.max(0, percent));
  return (
    <div>
      <div
        className={cn(
          "flex items-center justify-between gap-1.5 text-text-muted",
          size === "sm" ? "mb-1.5 text-meta" : "mb-2 text-label",
        )}
      >
        <span>{label}</span>
        <span className="text-text-secondary tabular-nums">{value}</span>
      </div>
      {/* The value is in the text above; the bar is decoration. */}
      <div
        aria-hidden
        className={cn("overflow-hidden rounded-full bg-meter-track", size === "sm" ? "h-[3px]" : "h-1")}
      >
        <div
          className={cn("h-full rounded-full transition-[width] duration-500", FILL[tone])}
          style={{ width: `${clamped}%` }}
        />
      </div>
    </div>
  );
}
