import { useNavigate } from "@tanstack/react-router";
import { LogOut } from "lucide-react";

import {
  CommandDialog,
  CommandEmpty,
  CommandGroup,
  CommandInput,
  CommandItem,
  CommandList,
  CommandShortcut,
} from "@/components/ui/command";

import { Role } from "@/gen/nethernode/v1/auth_pb";
import { useCurrentUser } from "@/lib/session";

import { ALL_NAV } from "./nav";
import { useSignOut } from "./use-sign-out";

/**
 * ⌘K / Ctrl+K command palette. Navigation for now; actions arrive with later
 * features. Loaded lazily by the app shell on first use.
 */
export default function CommandPalette({
  open,
  onOpenChange,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  const navigate = useNavigate();
  const signOut = useSignOut();
  const owner = useCurrentUser().role === Role.OWNER;

  const run = (action: () => unknown) => {
    onOpenChange(false);
    void action();
  };

  return (
    <CommandDialog
      open={open}
      onOpenChange={onOpenChange}
      title="Command palette"
      description="Jump to a page or run an action."
      showCloseButton={false}
      className="w-[520px] p-0"
    >
      <CommandInput placeholder="Type a command or search…" />
      <CommandList>
        <CommandEmpty>No matching commands.</CommandEmpty>
        <CommandGroup heading="Go to">
          {ALL_NAV.filter((item) => owner || !item.ownerOnly).map(({ to, label, icon: Icon }) => (
            <CommandItem key={to} value={`Go to ${label}`} onSelect={() => run(() => navigate({ to }))}>
              <Icon />
              {label}
            </CommandItem>
          ))}
        </CommandGroup>
        <CommandGroup heading="Account">
          <CommandItem value="Sign out" onSelect={() => run(signOut)}>
            <LogOut />
            Sign out
          </CommandItem>
        </CommandGroup>
      </CommandList>
      <div className="flex items-center justify-end gap-1.5 border-divider border-t px-3 py-2 text-meta text-text-muted">
        Toggle with <CommandShortcut className="ml-0">⌘K</CommandShortcut>
      </div>
    </CommandDialog>
  );
}
