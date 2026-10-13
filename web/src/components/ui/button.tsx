import { cva, type VariantProps } from "class-variance-authority";
import { Slot } from "radix-ui";
import type * as React from "react";

import { cn } from "@/lib/utils";

/**
 * Buttons from the UI reference: quiet (default), primary (one per view),
 * subtle (transparent), danger, and the compact `mini` size used on cards.
 */
const buttonVariants = cva(
  [
    "inline-flex shrink-0 items-center justify-center gap-2 whitespace-nowrap rounded-md border font-[550] text-label",
    "transition-colors duration-(--nn-transition) ease-out",
    "focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-primary",
    "disabled:cursor-not-allowed disabled:opacity-50 aria-disabled:cursor-not-allowed aria-disabled:opacity-50",
    "[&_svg]:pointer-events-none [&_svg]:shrink-0 [&_svg:not([class*='size-'])]:size-[15px]",
  ],
  {
    variants: {
      variant: {
        default:
          "border-border bg-button-quiet text-text-primary hover:not-disabled:border-border-strong hover:not-disabled:bg-button-quiet-hover",
        primary:
          "border-primary bg-primary text-primary-foreground hover:not-disabled:border-primary-hover hover:not-disabled:bg-primary-hover active:not-disabled:bg-primary-pressed",
        subtle: "border-border bg-transparent text-text-primary hover:not-disabled:bg-button-quiet-hover",
        ghost:
          "border-transparent bg-transparent text-text-secondary hover:not-disabled:bg-surface-elevated hover:not-disabled:text-text-primary",
        danger: "border-button-danger-border bg-button-danger text-danger hover:not-disabled:border-danger",
        accent:
          "border-primary/40 bg-nav-active text-nav-active-foreground hover:not-disabled:border-primary/70",
      },
      size: {
        default: "h-[35px] px-[13px]",
        mini: "h-[27px] gap-[5px] rounded-sm px-[9px] text-meta [&_svg:not([class*='size-'])]:size-3",
        lg: "h-10 px-4 text-nav",
        icon: "size-[33px] p-0 [&_svg:not([class*='size-'])]:size-[18px]",
      },
    },
    defaultVariants: { variant: "default", size: "default" },
  },
);

export interface ButtonProps extends React.ComponentProps<"button">, VariantProps<typeof buttonVariants> {
  /** Render the child element (e.g. a router `Link`) with button styling. */
  asChild?: boolean;
}

function Button({ className, variant, size, asChild = false, type, ...props }: ButtonProps) {
  const Comp = asChild ? Slot.Root : "button";
  return (
    <Comp
      data-slot="button"
      className={cn(buttonVariants({ variant, size }), className)}
      type={asChild ? undefined : (type ?? "button")}
      {...props}
    />
  );
}

export { Button, buttonVariants };
