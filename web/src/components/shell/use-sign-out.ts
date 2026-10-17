import { useQueryClient } from "@tanstack/react-query";
import { useNavigate } from "@tanstack/react-router";

import { authClient, errorMessage } from "@/lib/api";
import { type Session, sessionQuery } from "@/lib/session";
import { withToast } from "@/lib/toast";

/** Ends the session, then routes to the login page. */
export function useSignOut() {
  const queryClient = useQueryClient();
  const navigate = useNavigate();
  return async () => {
    try {
      await authClient.logout({});
    } catch (error) {
      const message = `Couldn't sign out. ${errorMessage(error)}`;
      withToast((toast) => toast.error(message, { duration: Number.POSITIVE_INFINITY }));
      return;
    }
    const anonymous: Session = { status: "anonymous", setupRequired: false };
    queryClient.clear();
    queryClient.setQueryData(sessionQuery.queryKey, anonymous);
    await navigate({ to: "/login" });
  };
}
