import { useQuery } from "@tanstack/react-query";
import { createFileRoute } from "@tanstack/react-router";
import { Box, LogOut, UserRound } from "lucide-react";
import type * as React from "react";
import { lazy, Suspense } from "react";

import { Footer, PageHeader, Panel, PanelHeader } from "@/components/page";
import { useSignOut } from "@/components/shell/use-sign-out";
import { Button } from "@/components/ui/button";
import { Role } from "@/gen/nethernode/v1/auth_pb";
import { healthQuery } from "@/lib/health";
import { ROLE_LABELS, useCanManage, useCurrentUser } from "@/lib/session";
import { useDocumentTitle } from "@/lib/use-document-title";

export const Route = createFileRoute("/_app/settings")({ component: SettingsPage });

const SecurityPanel = lazy(() => import("@/features/account/security-panel"));
const DiscordPanel = lazy(() => import("@/features/notifications/discord-panel"));
const InstallCountPanel = lazy(() => import("@/features/install-count/install-count-panel"));

const CurseForgeKey = lazy(() =>
  import("@/features/catalog/curseforge-key").then((m) => ({ default: m.CurseForgeKey })),
);

const dateFormat = new Intl.DateTimeFormat(undefined, { dateStyle: "medium", timeStyle: "short" });

function formatTime(ms: bigint | undefined): string {
  return ms === undefined || ms === 0n ? "Never" : dateFormat.format(new Date(Number(ms)));
}

function SettingsPage() {
  useDocumentTitle("Settings");
  const user = useCurrentUser();
  const signOut = useSignOut();
  const health = useQuery(healthQuery);
  const canManage = useCanManage();

  return (
    <>
      <PageHeader
        eyebrow="Settings"
        title="Make it yours."
        description="Simple defaults, fewer things to worry about."
      />
      <div className="flex max-w-[720px] flex-col gap-[18px]">
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

        <Suspense fallback={<div className="h-48 animate-pulse rounded-lg bg-surface-subtle" />}>
          <SecurityPanel />
        </Suspense>

        {canManage && (
          <Suspense fallback={<div className="h-48 animate-pulse rounded-lg bg-surface-subtle" />}>
            <DiscordPanel />
          </Suspense>
        )}

        {canManage && (
          <Panel>
            <PanelHeader title="Modpacks and mods" icon={<Box />} />
            <div className="py-[15px] first-of-type:pt-0 pb-0">
              <p className="font-[550] text-label">CurseForge API key</p>
              <p className="mt-[3px] mb-3 text-meta text-text-muted">
                Lets NetherNode search CurseForge and install from it. It's stored encrypted and passed to
                CurseForge modpack servers.
              </p>
              <Suspense fallback={<div className="h-9 animate-pulse rounded-md bg-surface-subtle" />}>
                <CurseForgeKey />
              </Suspense>
            </div>
          </Panel>
        )}

        {user.role === Role.OWNER && (
          <Suspense fallback={<div className="h-28 animate-pulse rounded-lg bg-surface-subtle" />}>
            <InstallCountPanel />
          </Suspense>
        )}
      </div>
      <Footer version={health.data?.version} />
    </>
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
