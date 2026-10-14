import { LogOut } from "lucide-react";

import { useSignOut } from "@/components/shell/use-sign-out";
import { Button } from "@/components/ui/button";
import { useCurrentUser } from "@/lib/session";
import { useDocumentTitle } from "@/lib/use-document-title";

import { AuthLayout } from "./auth-layout";
import { ChangePasswordForm } from "./change-password-form";

/** Shown instead of the app until a one-time password is replaced. */
export default function PasswordChangeRequired() {
  useDocumentTitle("Choose a password");
  const user = useCurrentUser();
  const signOut = useSignOut();
  return (
    <AuthLayout
      title="Choose your password"
      description={`Welcome, ${user.username}. You signed in with a one-time password; choose your own to continue.`}
    >
      <ChangePasswordForm currentLabel="One-time password">
        <Button variant="ghost" onClick={() => void signOut()}>
          <LogOut />
          Sign out
        </Button>
      </ChangePasswordForm>
    </AuthLayout>
  );
}
