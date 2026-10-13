import {
  Box,
  Cpu,
  History,
  LayoutGrid,
  type LucideIcon,
  Network,
  Server,
  Settings,
  Users,
} from "lucide-react";

export interface NavItem {
  to: "/" | "/servers" | "/players" | "/proxy" | "/modpacks" | "/backups" | "/host" | "/settings";
  label: string;
  icon: LucideIcon;
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
  { to: "/settings", label: "Settings", icon: Settings },
];

export const ALL_NAV = [...WORKSPACE_NAV, ...SYSTEM_NAV];

/** The nav item for a path, matching nested paths to their section. */
export function navItemFor(pathname: string): NavItem | undefined {
  if (pathname === "/") return WORKSPACE_NAV[0];
  return ALL_NAV.find((item) => item.to !== "/" && pathname.startsWith(item.to));
}
