import { createFileRoute, redirect } from "@tanstack/react-router";

import { AuthLayout } from "@/features/auth/auth-layout";
import { SetupForm } from "@/features/auth/setup-form";
import { sessionQuery } from "@/lib/session";
import { useDocumentTitle } from "@/lib/use-document-title";

export const Route = createFileRoute("/setup")({
  beforeLoad: async ({ context }) => {
    const session = await context.queryClient.ensureQueryData(sessionQuery);
    if (session.status === "authenticated") throw redirect({ to: "/" });
    if (!session.setupRequired) throw redirect({ to: "/login" });
  },
  component: SetupPage,
});

function SetupPage() {
  useDocumentTitle("Set up");
  return (
    <AuthLayout
      title="Create the owner account"
      description="This account has full control of NetherNode. You can invite others later."
      footer="Welcome to NetherNode. Your worlds, under control."
    >
      <SetupForm />
    </AuthLayout>
  );
}
