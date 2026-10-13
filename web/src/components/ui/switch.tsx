import { Switch as SwitchPrimitive } from "radix-ui";
import type * as React from "react";

import { cn } from "@/lib/utils";

/**
 * Toggle switch from the UI reference (35×20). The off track uses
 * border-strong so the control's boundary meets 3:1 contrast.
 */
export function Switch({ className, ...props }: React.ComponentProps<typeof SwitchPrimitive.Root>) {
  return (
    <SwitchPrimitive.Root
      data-slot="switch"
      className={cn(
        "inline-flex h-5 w-[35px] shrink-0 items-center rounded-full p-[3px] transition-colors duration-(--nn-transition)",
        "bg-border-strong data-[state=checked]:bg-primary",
        "focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-primary",
        "disabled:cursor-not-allowed disabled:opacity-50",
        className,
      )}
      {...props}
    >
      <SwitchPrimitive.Thumb
        className={cn(
          "block size-3.5 rounded-full bg-text-primary transition-transform duration-200",
          "data-[state=checked]:translate-x-[15px] data-[state=checked]:bg-primary-foreground",
        )}
      />
    </SwitchPrimitive.Root>
  );
}
