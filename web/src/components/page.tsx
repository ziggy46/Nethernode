import { useQuery } from "@tanstack/react-query";
import { ShieldCheck } from "lucide-react";
import type * as React from "react";

import { healthQuery } from "@/lib/health";
import { cn } from "@/lib/utils";

/** Eyebrow, heading, description, and top-right actions. */
export function PageHeader({
  eyebrow,
  title,
  description,
  actions,
}: {
  eyebrow: string;
  title: React.ReactNode;
  description: React.ReactNode;
  actions?: React.ReactNode;
}) {
  return (
    <div className="mb-[25px] flex items-center justify-between gap-5 below-md:items-start below-md:gap-[15px]">
      <div className="min-w-0">
        <p className="mb-1.5 font-[650] text-eyebrow text-primary uppercase">{eyebrow}</p>
        <h1 className="mb-[7px] font-[650] text-page below-md:text-[25px]">{title}</h1>
        <p className="text-label text-text-muted">{description}</p>
      </div>
      {actions && <div className="flex shrink-0 items-center gap-[9px]">{actions}</div>}
    </div>
  );
}

/** A quiet bordered card on surface-subtle. */
export function Panel({ className, ...props }: React.ComponentProps<"section">) {
  return (
    <section
      className={cn("rounded-lg border border-divider bg-surface-subtle p-[19px]", className)}
      {...props}
    />
  );
}

export function PanelHeader({ title, icon }: { title: string; icon?: React.ReactNode }) {
  return (
    <div className="mb-[17px] flex items-center justify-between">
      <h2 className="font-semibold text-label tracking-[-0.3px]">{title}</h2>
      {icon && <span className="text-text-muted [&_svg]:size-[15px]">{icon}</span>}
    </div>
  );
}

/**
 * An honest empty or unavailable state: muted icon, a clear title, and an
 * optional action.
 */
export function EmptyState({
  icon,
  title,
  children,
  action,
  className,
  compact = false,
}: {
  icon: React.ReactNode;
  title: string;
  children?: React.ReactNode;
  action?: React.ReactNode;
  className?: string;
  compact?: boolean;
}) {
  return (
    <div
      className={cn(
        "flex flex-col items-center text-center",
        compact ? "gap-2 py-4" : "gap-3 px-6 py-12",
        className,
      )}
    >
      <span
        className={cn("text-text-muted opacity-60", compact ? "[&_svg]:size-7" : "[&_svg]:size-12")}
        aria-hidden
      >
        {icon}
      </span>
      <p className={cn("font-semibold", compact ? "text-label" : "text-section")}>{title}</p>
      {children && (
        <p className={cn("max-w-[42ch] text-text-muted", compact ? "text-meta" : "text-label")}>{children}</p>
      )}
      {action && <div className="mt-2">{action}</div>}
    </div>
  );
}

/** Summary metric: label, icon, value with unit, and a supporting note. */
export function MetricCard({
  label,
  icon,
  value,
  unit,
  note,
}: {
  label: string;
  icon: React.ReactNode;
  value: React.ReactNode;
  unit?: React.ReactNode;
  note: React.ReactNode;
}) {
  return (
    <div className="relative min-w-0 overflow-hidden rounded-lg border border-divider bg-surface-subtle px-5 py-[17px] below-xl:p-[15px]">
      <div className="mb-[11px] flex items-center justify-between text-meta text-text-secondary">
        {label}
        <span className="text-text-muted [&_svg]:size-4" aria-hidden>
          {icon}
        </span>
      </div>
      <p className="font-[650] text-metric leading-[1.1] below-md:text-[24px]">
        {value}
        {unit && (
          <span className="ml-[5px] font-normal text-[13px] text-text-muted tracking-normal below-sm:text-meta">
            {unit}
          </span>
        )}
      </p>
      <p className="mt-[9px] text-meta text-text-muted">{note}</p>
    </div>
  );
}

export function SectionHeading({
  title,
  count,
  caption,
}: {
  title: string;
  count?: number;
  caption?: React.ReactNode;
}) {
  return (
    <div className="mb-[17px] flex items-center justify-between gap-3">
      <h2 className="font-semibold text-section">
        {title}
        {count !== undefined && (
          <span className="ml-[7px] font-normal text-label text-text-muted tracking-normal">{count}</span>
        )}
      </h2>
      {caption && <span className="text-meta text-text-muted">{caption}</span>}
    </div>
  );
}

/** The line at the bottom of every page, kept at the bottom of the window on short pages. */
export function Footer() {
  const version = useQuery(healthQuery).data?.version;
  return (
    <footer className="mt-auto flex flex-wrap items-center justify-between gap-2.5 pt-7 text-meta text-text-muted">
      <span className="flex items-center gap-[5px]">
        <ShieldCheck className="size-3" aria-hidden />
        NetherNode · Your worlds, under control.
      </span>
      {version && <span>v{version}</span>}
    </footer>
  );
}
