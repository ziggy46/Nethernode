import { useQuery } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { Box, CircleAlert, CircleCheck, Settings } from "lucide-react";

import { Logo, Wordmark } from "@/components/brand";
import { healthQuery } from "@/lib/health";
import { ROLE_LABELS, useCurrentUser } from "@/lib/session";
import { cn } from "@/lib/utils";

import { AccountMenu } from "./account-menu";
import { Avatar } from "./avatar";
import { type NavItem, SYSTEM_NAV, WORKSPACE_NAV } from "./nav";

export function Sidebar({ open, onNavigate }: { open: boolean; onNavigate: () => void }) {
  const user = useCurrentUser();
  return (
    <aside
      id="sidebar"
      aria-label="Sidebar"
      className={cn(
        "fixed inset-y-0 left-0 z-40 flex w-(--nn-sidebar-width) flex-col overflow-y-auto border-divider border-r bg-sidebar px-[17px] pt-[27px] pb-[18px]",
        "below-xl:px-3",
        "below-md:shadow-overlay below-md:transition-transform below-md:duration-200",
        !open && "below-md:-translate-x-full below-md:invisible",
      )}
    >
      <Link
        to="/"
        onClick={onNavigate}
        aria-label="NetherNode overview"
        className="flex items-center gap-2.5 rounded-md px-2 pb-[30px] below-md:pb-[22px]"
      >
        <Logo decorative className="h-[46px] w-[42px] shrink-0" />
        <span className="min-w-0">
          <Wordmark className="block leading-tight" />
          <span className="block text-[10px] text-text-muted uppercase tracking-[2px]">
            Server management
          </span>
        </span>
      </Link>

      <div className="mb-[27px] flex items-center gap-2.5 rounded-[9px] border border-divider bg-button-quiet p-[11px]">
        <span className="grid size-8 shrink-0 place-items-center rounded-md bg-nav-active text-nav-active-foreground">
          <Box className="size-[18px]" aria-hidden />
        </span>
        <span className="min-w-0">
          <span className="block truncate font-semibold text-label">My workspace</span>
          <span className="block truncate text-meta text-text-muted">Self-hosted panel</span>
        </span>
      </div>

      <NavSection label="Workspace" items={WORKSPACE_NAV} onNavigate={onNavigate} />
      <NavSection label="System" items={SYSTEM_NAV} onNavigate={onNavigate} />

      <div className="mt-auto">
        <ConnectionCard />
        <div className="flex items-center gap-2.5 border-divider border-t px-[9px] pt-[17px]">
          <Avatar name={user.username} />
          <span className="min-w-0">
            <span className="block truncate font-semibold text-label">{user.username}</span>
            <span className="block truncate text-meta text-text-muted">{ROLE_LABELS[user.role]}</span>
          </span>
          <AccountMenu align="start" side="top">
            <button
              type="button"
              aria-label="Account menu"
              className="ml-auto grid size-8 place-items-center rounded-md text-text-muted transition-colors hover:bg-nav-hover hover:text-text-primary"
            >
              <Settings className="size-[18px]" />
            </button>
          </AccountMenu>
        </div>
      </div>
    </aside>
  );
}

function NavSection({
  label,
  items,
  onNavigate,
}: {
  label: string;
  items: NavItem[];
  onNavigate: () => void;
}) {
  return (
    <>
      <p className="mb-[9px] px-[13px] font-[650] text-[10px] text-text-muted uppercase tracking-[1.5px]">
        {label}
      </p>
      <nav aria-label={`${label} navigation`} className="mb-[27px] flex flex-col gap-1">
        {items.map(({ to, label: text, icon: Icon }) => (
          <Link
            key={to}
            to={to}
            onClick={onNavigate}
            activeOptions={{ exact: to === "/" }}
            className={cn(
              "flex items-center gap-3 rounded-md px-[13px] py-[11px] text-nav text-text-secondary transition-colors duration-(--nn-transition)",
              "hover:bg-nav-hover hover:text-text-primary",
              "data-[status=active]:bg-nav-active data-[status=active]:text-nav-active-foreground data-[status=active]:shadow-[inset_3px_0_var(--color-primary)]",
            )}
          >
            <Icon className="size-[18px]" aria-hidden />
            {text}
          </Link>
        ))}
      </nav>
    </>
  );
}

/** Live panel connectivity, from `/healthz`. */
function ConnectionCard() {
  const health = useQuery(healthQuery);
  const connected = health.isSuccess;
  const label = health.isPending ? "Connecting…" : connected ? "Panel connected" : "Connection lost";
  return (
    <div className="mb-[17px] rounded-[9px] border border-divider p-[13px] text-label" role="status">
      <div className="mb-[7px] flex items-center justify-between">
        <span className="flex items-center gap-1.5 font-semibold">
          <span
            aria-hidden
            className={cn(
              "inline-block size-1.5 rounded-full",
              connected ? "bg-success shadow-[0_0_8px_var(--color-success-subtle)]" : "bg-text-muted",
              health.isError && "bg-danger",
            )}
          />
          {label}
        </span>
        {connected ? (
          <CircleCheck className="size-[13px] text-success" aria-hidden />
        ) : (
          health.isError && <CircleAlert className="size-[13px] text-danger" aria-hidden />
        )}
      </div>
      <div className="flex justify-between text-meta text-text-muted">
        <span>This host</span>
        {health.data && <span>v{health.data.version}</span>}
      </div>
    </div>
  );
}
