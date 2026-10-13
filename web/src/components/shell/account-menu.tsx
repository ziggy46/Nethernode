import { Link } from "@tanstack/react-router";
import { LogOut, Settings } from "lucide-react";
import type * as React from "react";

import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { ROLE_LABELS, useCurrentUser } from "@/lib/session";

import { useSignOut } from "./use-sign-out";

/** Account actions, opened from the sidebar user row or the top-bar avatar. */
export function AccountMenu({
  children,
  align = "end",
  side = "bottom",
}: {
  children: React.ReactNode;
  align?: "start" | "end";
  side?: "top" | "bottom";
}) {
  const user = useCurrentUser();
  const signOut = useSignOut();
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>{children}</DropdownMenuTrigger>
      <DropdownMenuContent align={align} side={side} sideOffset={8} className="min-w-52">
        <DropdownMenuLabel className="font-normal">
          <span className="block font-semibold text-label">{user.username}</span>
          <span className="block text-meta text-text-muted">{ROLE_LABELS[user.role]}</span>
        </DropdownMenuLabel>
        <DropdownMenuSeparator />
        <DropdownMenuItem asChild>
          <Link to="/settings">
            <Settings />
            Settings
          </Link>
        </DropdownMenuItem>
        <DropdownMenuItem onSelect={() => void signOut()}>
          <LogOut />
          Sign out
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
