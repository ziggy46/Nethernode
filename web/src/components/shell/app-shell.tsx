import { useQuery } from "@tanstack/react-query";
import { useRouterState } from "@tanstack/react-router";
import { Bell, Menu, Search, X } from "lucide-react";
import type * as React from "react";
import { lazy, Suspense, useCallback, useEffect, useState } from "react";

import { Button } from "@/components/ui/button";
import { useServerEvents } from "@/lib/events";
import { healthQuery } from "@/lib/health";
import { useCurrentUser } from "@/lib/session";
import { cn } from "@/lib/utils";

import { AccountMenu } from "./account-menu";
import { Avatar } from "./avatar";
import { navItemFor } from "./nav";
import { Sidebar } from "./sidebar";

const CommandPalette = lazy(() => import("./command-palette"));
const NotificationsDialog = lazy(() => import("./notifications-dialog"));

/** Fixed sidebar, top bar, and scrolling content, per the UI reference. */
export function AppShell({ children }: { children: React.ReactNode }) {
  useServerEvents();
  const [navOpen, setNavOpen] = useState(false);
  const [paletteOpen, setPaletteOpen] = useState(false);
  // The palette's code is fetched the first time it's opened.
  const [paletteLoaded, setPaletteLoaded] = useState(false);
  const closeNav = useCallback(() => setNavOpen(false), []);
  const openPalette = useCallback(() => {
    setPaletteLoaded(true);
    setPaletteOpen(true);
  }, []);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key.toLowerCase() === "k" && (e.metaKey || e.ctrlKey)) {
        e.preventDefault();
        setPaletteLoaded(true);
        setPaletteOpen((v) => !v);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  useEffect(() => {
    if (!navOpen) return;
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && setNavOpen(false);
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [navOpen]);

  return (
    <div className="min-h-dvh">
      <a
        href="#main"
        className="sr-only z-50 rounded-md bg-primary px-3 py-2 text-primary-foreground focus:not-sr-only focus:fixed focus:top-3 focus:left-3"
      >
        Skip to content
      </a>
      <Sidebar open={navOpen} onNavigate={closeNav} />
      {navOpen && (
        <button
          type="button"
          aria-label="Close navigation"
          tabIndex={-1}
          className="fixed inset-0 z-30 hidden bg-overlay below-md:block"
          onClick={closeNav}
        />
      )}
      <div className="ml-(--nn-sidebar-width) below-md:ml-0">
        <TopBar navOpen={navOpen} onToggleNav={() => setNavOpen((v) => !v)} onOpenPalette={openPalette} />
        <main
          id="main"
          tabIndex={-1}
          className="mx-auto max-w-[1700px] px-(--nn-main-padding-x) pt-(--nn-main-padding-y) pb-[26px] outline-none"
        >
          {children}
        </main>
      </div>
      {paletteLoaded && (
        <Suspense fallback={null}>
          <CommandPalette open={paletteOpen} onOpenChange={setPaletteOpen} />
        </Suspense>
      )}
    </div>
  );
}

function TopBar({
  navOpen,
  onToggleNav,
  onOpenPalette,
}: {
  navOpen: boolean;
  onToggleNav: () => void;
  onOpenPalette: () => void;
}) {
  const user = useCurrentUser();
  const pathname = useRouterState({ select: (s) => s.location.pathname });
  const page = navItemFor(pathname)?.label ?? "Overview";
  const health = useQuery(healthQuery);
  const [activityOpen, setActivityOpen] = useState(false);
  const [activityLoaded, setActivityLoaded] = useState(false);

  return (
    <header className="flex h-(--nn-topbar-height) items-center gap-3 border-divider border-b bg-background px-(--nn-main-padding-x)">
      <Button
        variant="ghost"
        size="icon"
        className="hidden border-divider below-md:inline-flex"
        aria-label={navOpen ? "Close navigation" : "Open navigation"}
        aria-expanded={navOpen}
        aria-controls="sidebar"
        onClick={onToggleNav}
      >
        {navOpen ? <X /> : <Menu />}
      </Button>
      <nav aria-label="Breadcrumb" className="flex min-w-0 items-center gap-2.5 text-label text-text-muted">
        <span aria-current="page" className="truncate font-medium text-text-secondary">
          {page}
        </span>
      </nav>
      <div className="ml-auto flex items-center gap-[18px] below-md:gap-2.5">
        <span
          className="flex items-center gap-1.5 text-meta text-text-secondary below-lg:hidden"
          role="status"
        >
          <span
            aria-hidden
            className={cn(
              "size-1.5 rounded-full",
              health.isSuccess ? "bg-success" : health.isError ? "bg-danger" : "bg-text-muted",
            )}
          />
          {health.isSuccess ? "Panel online" : health.isError ? "Panel unreachable" : "Checking…"}
        </span>
        <Button
          variant="ghost"
          size="icon"
          className="border-divider"
          aria-label="Open command palette (⌘K)"
          title="Command palette (⌘K)"
          onClick={onOpenPalette}
        >
          <Search />
        </Button>
        <Button
          variant="ghost"
          size="icon"
          className="border-divider"
          aria-label="View notifications"
          onClick={() => {
            setActivityLoaded(true);
            setActivityOpen(true);
          }}
        >
          <Bell />
        </Button>
        <AccountMenu>
          <button type="button" aria-label="Account menu" className="rounded-full">
            <Avatar name={user.username} className="size-[29px]" />
          </button>
        </AccountMenu>
      </div>
      {activityLoaded && (
        <Suspense fallback={null}>
          <NotificationsDialog open={activityOpen} onOpenChange={setActivityOpen} />
        </Suspense>
      )}
    </header>
  );
}
