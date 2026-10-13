import { cn } from "@/lib/utils";

/** Initial-letter avatar in the warm nav-active tint. */
export function Avatar({ name, className }: { name: string; className?: string }) {
  return (
    <span
      aria-hidden
      className={cn(
        "grid size-8 shrink-0 place-items-center rounded-full border border-primary/35 bg-nav-active font-[650] text-label text-nav-active-foreground",
        className,
      )}
    >
      {name.slice(0, 1).toUpperCase() || "?"}
    </span>
  );
}
