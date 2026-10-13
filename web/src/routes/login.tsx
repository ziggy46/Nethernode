import { createFileRoute, redirect } from "@tanstack/react-router";

import { AuthLayout } from "@/features/auth/auth-layout";
import { LoginForm } from "@/features/auth/login-form";
import { sessionQuery } from "@/lib/session";
import { useDocumentTitle } from "@/lib/use-document-title";

export const Route = createFileRoute("/login")({
  validateSearch: (search: Record<string, unknown>): { redirect?: string } =>
    typeof search.redirect === "string" ? { redirect: search.redirect } : {},
  beforeLoad: async ({ context }) => {
    const session = await context.queryClient.ensureQueryData(sessionQuery);
    if (session.status === "authenticated") throw redirect({ to: "/" });
    if (session.setupRequired) throw redirect({ to: "/setup" });
  },
  component: LoginPage,
});

function LoginPage() {
  useDocumentTitle("Sign in");
  const { redirect: redirectTo } = Route.useSearch();
  return (
    <AuthLayout title="Welcome back" description="Sign in to manage your worlds.">
      <LoginForm redirectTo={redirectTo} />
    </AuthLayout>
  );
}
