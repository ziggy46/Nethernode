import { CircleAlert } from "lucide-react";
import type * as React from "react";
import { useId } from "react";

import { cn } from "@/lib/utils";

/** Text input. Outlines use border-strong to meet the 3:1 boundary rule. */
export function Input({ className, ...props }: React.ComponentProps<"input">) {
  return (
    <input
      data-slot="input"
      className={cn(
        "block h-10 w-full rounded-md border border-border-strong bg-sidebar px-3 text-nav text-text-primary",
        "placeholder:text-text-muted",
        "transition-colors duration-(--nn-transition) hover:border-text-muted",
        "focus-visible:border-primary focus-visible:outline-2 focus-visible:outline-primary focus-visible:outline-offset-2",
        "aria-invalid:border-danger disabled:cursor-not-allowed disabled:opacity-50",
        className,
      )}
      {...props}
    />
  );
}

/** Label, input, and a hint or error underneath, wired up for assistive tech. */
export function Field({
  label,
  hint,
  error,
  className,
  ...inputProps
}: React.ComponentProps<"input"> & { label: string; hint?: string; error?: string | undefined }) {
  const id = useId();
  const describedBy = error ? `${id}-error` : hint ? `${id}-hint` : undefined;
  return (
    <div className={cn("mb-[17px]", className)}>
      <label htmlFor={id} className="mb-[7px] block text-label text-text-secondary">
        {label}
      </label>
      <Input id={id} aria-invalid={error ? true : undefined} aria-describedby={describedBy} {...inputProps} />
      {error ? (
        <p id={`${id}-error`} className="mt-1.5 text-meta text-danger">
          {error}
        </p>
      ) : (
        hint && (
          <p id={`${id}-hint`} className="mt-1.5 text-meta text-text-muted">
            {hint}
          </p>
        )
      )}
    </div>
  );
}

/** A native select styled like the inputs (keeps platform keyboard behavior). */
export function Select({ className, ...props }: React.ComponentProps<"select">) {
  return (
    <select
      data-slot="select"
      className={cn(
        "block h-10 w-full rounded-md border border-border-strong bg-sidebar px-3 text-nav text-text-primary",
        "transition-colors duration-(--nn-transition) hover:border-text-muted",
        "focus-visible:border-primary focus-visible:outline-2 focus-visible:outline-primary focus-visible:outline-offset-2",
        "disabled:cursor-not-allowed disabled:opacity-50",
        className,
      )}
      {...props}
    />
  );
}

/** Inline error message for a failed operation. Stays until the user acts. */
export function ErrorAlert({ children, className }: { children: React.ReactNode; className?: string }) {
  return (
    <div
      role="alert"
      className={cn(
        "mb-[17px] flex items-start gap-2.5 rounded-md border border-danger/35 bg-danger-subtle px-3 py-2.5 text-label text-danger",
        className,
      )}
    >
      <CircleAlert className="mt-px size-4 shrink-0" aria-hidden />
      <span>{children}</span>
    </div>
  );
}
