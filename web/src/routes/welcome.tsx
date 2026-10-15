import { createFileRoute, redirect } from "@tanstack/react-router";

import { AuthLayout } from "@/features/auth/auth-layout";
import { WelcomeDomain } from "@/features/auth/welcome-domain";
import { Role } from "@/gen/nethernode/v1/auth_pb";
import { sessionQuery } from "@/lib/session";
import { useDocumentTitle } from "@/lib/use-document-title";

/** The step after first-run setup: an optional domain. */
export const Route = createFileRoute("/welcome")({
  beforeLoad: async ({ context }) => {
    const session = await context.queryClient.ensureQueryData(sessionQuery);
    if (session.status !== "authenticated") throw redirect({ to: "/login" });
    if (session.user.role < Role.ADMIN) throw redirect({ to: "/" });
  },
  component: WelcomePage,
});

function WelcomePage() {
  useDocumentTitle("Welcome");
  return (
    <AuthLayout
      title="Do you have a domain?"
      description="With a domain, each server can have its own name for players to join with. Optional: servers can also use a port."
      footer="Welcome to NetherNode. Your worlds, under control."
    >
      <WelcomeDomain />
    </AuthLayout>
  );
}
