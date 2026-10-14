import {
  Box,
  ClipboardList,
  Cpu,
  History,
  LayoutGrid,
  type LucideIcon,
  Network,
  Server,
  Settings,
  UserCog,
  Users,
} from "lucide-react";

import { Role } from "@/gen/nethernode/v1/auth_pb";

export interface NavItem {
  to:
    | "/"
    | "/servers"
    | "/players"
    | "/proxy"
    | "/modpacks"
    | "/backups"
    | "/host"
    | "/audit"
    | "/users"
    | "/settings";
  label: string;
  icon: LucideIcon;
  /** Shown only to this role and above. */
  minRole?: Role;
}

export const WORKSPACE_NAV: NavItem[] = [
  { to: "/", label: "Overview", icon: LayoutGrid },
  { to: "/servers", label: "Servers", icon: Server },
  { to: "/players", label: "Players", icon: Users },
  { to: "/proxy", label: "Proxy & routes", icon: Network },
  { to: "/modpacks", label: "Modpacks", icon: Box },
  { to: "/backups", label: "Backups", icon: History },
];

export const SYSTEM_NAV: NavItem[] = [
  { to: "/host", label: "Host node", icon: Cpu },
  { to: "/audit", label: "Audit log", icon: ClipboardList, minRole: Role.ADMIN },
  { to: "/users", label: "Users", icon: UserCog, minRole: Role.OWNER },
  { to: "/settings", label: "Settings", icon: Settings },
];

export const ALL_NAV = [...WORKSPACE_NAV, ...SYSTEM_NAV];

/** Whether `role` sees `item`. */
export function canSee(item: NavItem, role: Role): boolean {
  return item.minRole === undefined || role >= item.minRole;
}

/** The nav item for a path, matching nested paths to their section. */
export function navItemFor(pathname: string): NavItem | undefined {
  if (pathname === "/") return WORKSPACE_NAV[0];
  return ALL_NAV.find((item) => item.to !== "/" && pathname.startsWith(item.to));
}
