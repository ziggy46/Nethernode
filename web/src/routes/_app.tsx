import { createFileRoute, Outlet, redirect } from "@tanstack/react-router";
import { lazy, Suspense } from "react";

import { AppShell } from "@/components/shell/app-shell";
import { UploadTrayHost } from "@/features/uploads/tray-host";
import { sessionQuery, useCurrentUser } from "@/lib/session";

const PasswordChangeRequired = lazy(() => import("@/features/auth/password-change-required"));

/** Authenticated layout: every page inside requires a session. */
export const Route = createFileRoute("/_app")({
  beforeLoad: async ({ context, location }) => {
    const session = await context.queryClient.ensureQueryData(sessionQuery);
    if (session.status === "anonymous") {
      if (session.setupRequired) throw redirect({ to: "/setup" });
      throw redirect({
        to: "/login",
        search: location.href === "/" ? {} : { redirect: location.href },
      });
    }
  },
  component: AppLayout,
});

function AppLayout() {
  const user = useCurrentUser();
  // Nothing else works until a one-time password is replaced, so the shell
  // (and its event stream) waits too.
  if (user.mustChangePassword) {
    return (
      <Suspense fallback={null}>
        <PasswordChangeRequired />
      </Suspense>
    );
  }
  return (
    <AppShell>
      <Outlet />
      <UploadTrayHost />
    </AppShell>
  );
}
