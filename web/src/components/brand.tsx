import { cn } from "@/lib/utils";

/** The NetherNode server-cube mark. Never stretched or clipped. */
export function Logo({ className, decorative = false }: { className?: string; decorative?: boolean }) {
  return (
    <img
      src="/icon-192.png"
      alt={decorative ? "" : "NetherNode obsidian and lava server logo"}
      className={cn("object-contain", className)}
      draggable={false}
    />
  );
}

/** "Nether" in light text and "Node" in orange. */
export function Wordmark({ className }: { className?: string }) {
  return (
    <span className={cn("font-[750] text-[20px] text-text-primary tracking-[-0.7px]", className)}>
      Nether<span className="text-primary">Node</span>
    </span>
  );
}
