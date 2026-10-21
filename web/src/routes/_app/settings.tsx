import { createFileRoute, Link } from "@tanstack/react-router";
import {
  Bell,
  Box,
  ChartNoAxesColumn,
  Cloud,
  Globe,
  HardDrive,
  LogOut,
  Palette,
  UserRound,
} from "lucide-react";
import type * as React from "react";
import { lazy, Suspense } from "react";

import { PageHeader, Panel, PanelHeader } from "@/components/page";
import { useSignOut } from "@/components/shell/use-sign-out";
import { Button } from "@/components/ui/button";
import { Role } from "@/gen/nethernode/v1/auth_pb";
import { ROLE_LABELS, useCanManage, useCurrentUser } from "@/lib/session";
import { useDocumentTitle } from "@/lib/use-document-title";
import { cn } from "@/lib/utils";

/** One page of Settings per category, at `/settings?section=…`. */
export const Route = createFileRoute("/_app/settings")({
  validateSearch: (search: Record<string, unknown>): { section?: string } =>
    typeof search.section === "string" ? { section: search.section } : {},
  component: SettingsPage,
});

const AppearancePanel = lazy(() => import("@/features/appearance/appearance-panel"));
const SecurityPanel = lazy(() => import("@/features/account/security-panel"));
const DiscordPanel = lazy(() => import("@/features/notifications/discord-panel"));
const UpdateCheckPanel = lazy(() => import("@/features/notifications/update-check-panel"));
const DomainsPanel = lazy(() => import("@/features/domains/domains-panel"));
const InstallCountPanel = lazy(() => import("@/features/install-count/install-count-panel"));
const CachePanel = lazy(() => import("@/features/storage/cache-panel"));
const CloudflarePanel = lazy(() => import("@/features/cloudflare/cloudflare-panel"));

const CurseForgeKey = lazy(() =>
  import("@/features/catalog/curseforge-key").then((m) => ({ default: m.CurseForgeKey })),
);

type Access = "everyone" | "admins" | "owners";

const SECTIONS: { id: string; label: string; icon: React.ReactNode; access: Access }[] = [
  { id: "account", label: "Account", icon: <UserRound />, access: "everyone" },
  { id: "appearance", label: "Appearance", icon: <Palette />, access: "everyone" },
  { id: "domains", label: "Domains", icon: <Globe />, access: "admins" },
  { id: "cloudflare", label: "Cloudflare", icon: <Cloud />, access: "admins" },
  { id: "notifications", label: "Notifications", icon: <Bell />, access: "admins" },
  { id: "mods", label: "Mods and modpacks", icon: <Box />, access: "admins" },
  { id: "storage", label: "Storage", icon: <HardDrive />, access: "admins" },
  { id: "stats", label: "Anonymous stats", icon: <ChartNoAxesColumn />, access: "owners" },
];

const dateFormat = new Intl.DateTimeFormat(undefined, { dateStyle: "medium", timeStyle: "short" });

function formatTime(ms: bigint | undefined): string {
  return ms === undefined || ms === 0n ? "Never" : dateFormat.format(new Date(Number(ms)));
}

function SettingsPage() {
  const user = useCurrentUser();
  const canManage = useCanManage();
  const { section } = Route.useSearch();
  const allowed = (access: Access) =>
    access === "everyone" || (access === "admins" ? canManage : user.role === Role.OWNER);
  const sections = SECTIONS.filter((s) => allowed(s.access));
  const current = sections.find((s) => s.id === section) ?? sections[0];
  useDocumentTitle(current && current.id !== "account" ? `${current.label} · Settings` : "Settings");

  return (
    <>
      <PageHeader
        eyebrow="Settings"
        title="Make it yours."
        description="Simple defaults, fewer things to worry about."
      />
      <div className="flex items-start gap-7 below-lg:flex-col below-lg:gap-4">
        <nav
          aria-label="Settings"
          className="sticky top-5 flex w-[210px] shrink-0 flex-col gap-0.5 below-lg:static below-lg:w-full below-lg:flex-row below-lg:overflow-x-auto"
        >
          {sections.map((s) => {
            const active = s.id === current?.id;
            return (
              <Link
                key={s.id}
                to="/settings"
                search={{ section: s.id }}
                aria-current={active ? "page" : undefined}
                className={cn(
                  "flex shrink-0 items-center gap-2.5 rounded-md px-3 py-2 text-nav transition-colors duration-(--nn-transition) [&_svg]:size-4",
                  active
                    ? "bg-surface-elevated text-text-primary"
                    : "text-text-secondary hover:bg-surface-subtle hover:text-text-primary",
                )}
              >
                {s.icon}
                {s.label}
              </Link>
            );
          })}
        </nav>
        <div className="flex w-full max-w-[720px] min-w-0 flex-col gap-[18px]">
          <Section id={current?.id ?? "account"} />
        </div>
      </div>
    </>
  );
}

function Loading({ height }: { height: string }) {
  return <div className={cn("animate-pulse rounded-lg bg-surface-subtle", height)} />;
}

function Section({ id }: { id: string }) {
  switch (id) {
    case "appearance":
      return (
        <Suspense fallback={<Loading height="h-24" />}>
          <AppearancePanel />
        </Suspense>
      );
    case "domains":
      return (
        <Suspense fallback={<Loading height="h-40" />}>
          <DomainsPanel />
        </Suspense>
      );
    case "cloudflare":
      return (
        <Suspense fallback={<Loading height="h-40" />}>
          <CloudflarePanel />
        </Suspense>
      );
    case "notifications":
      return (
        <Suspense fallback={<Loading height="h-48" />}>
          <UpdateCheckPanel />
          <DiscordPanel />
        </Suspense>
      );
    case "mods":
      return (
        <Panel>
          <PanelHeader title="Modpacks and mods" icon={<Box />} />
          <p className="font-[550] text-label">CurseForge API key</p>
          <p className="mt-[3px] mb-3 text-meta text-text-muted">
            Lets NetherNode search CurseForge and install from it. It's stored encrypted and passed to
            CurseForge modpack servers.
          </p>
          <Suspense fallback={<Loading height="h-9" />}>
            <CurseForgeKey />
          </Suspense>
        </Panel>
      );
    case "storage":
      return (
        <Suspense fallback={<Loading height="h-48" />}>
          <CachePanel />
        </Suspense>
      );
    case "stats":
      return (
        <Suspense fallback={<Loading height="h-28" />}>
          <InstallCountPanel />
        </Suspense>
      );
    default:
      return (
        <>
          <AccountPanel />
          <Suspense fallback={<Loading height="h-48" />}>
            <SecurityPanel />
          </Suspense>
        </>
      );
  }
}

function AccountPanel() {
  const user = useCurrentUser();
  const signOut = useSignOut();
  return (
    <Panel>
      <PanelHeader title="Your account" icon={<UserRound />} />
      <Row title="Username" description="Used to sign in.">
        <span className="font-mono text-label">{user.username}</span>
      </Row>
      <Row title="Role" description="Owners have full control, including users and metrics.">
        <span className="rounded-xs bg-surface-elevated px-1.5 py-px text-meta text-text-secondary">
          {ROLE_LABELS[user.role]}
        </span>
      </Row>
      <Row title="Member since" description="When this account was created.">
        <span className="text-label text-text-secondary">{formatTime(user.createdAtMs)}</span>
      </Row>
      <Row title="Session" description="Sign out of NetherNode on this device." last>
        <Button variant="subtle" onClick={() => void signOut()}>
          <LogOut />
          Sign out
        </Button>
      </Row>
    </Panel>
  );
}

function Row({
  title,
  description,
  children,
  last = false,
}: {
  title: string;
  description: string;
  children: React.ReactNode;
  last?: boolean;
}) {
  return (
    <div
      className={`flex items-center justify-between gap-5 py-[15px] first-of-type:pt-0 ${last ? "pb-0" : "border-divider border-b"}`}
    >
      <div className="min-w-0">
        <p className="font-[550] text-label">{title}</p>
        <p className="mt-[3px] text-meta text-text-muted">{description}</p>
      </div>
      <div className="shrink-0">{children}</div>
    </div>
  );
}
