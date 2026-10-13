import { queryOptions, useQuery, useQueryClient } from "@tanstack/react-query";

import { Role, type User } from "@/gen/nethernode/v1/auth_pb";
import { authClient, Code, ConnectError, SETUP_REQUIRED_HEADER } from "@/lib/api";

export type Session =
  | { status: "authenticated"; user: User }
  | { status: "anonymous"; setupRequired: boolean };

/** Resolves the current session. Network failures throw; 401s don't. */
export async function fetchSession(): Promise<Session> {
  try {
    const { user } = await authClient.me({});
    if (!user) throw new ConnectError("missing user in response", Code.Internal);
    return { status: "authenticated", user };
  } catch (error) {
    const err = ConnectError.from(error);
    if (err.code === Code.Unauthenticated) {
      return { status: "anonymous", setupRequired: err.metadata.get(SETUP_REQUIRED_HEADER) === "1" };
    }
    throw err;
  }
}

export const sessionQuery = queryOptions({
  queryKey: ["session"],
  queryFn: fetchSession,
  staleTime: 60_000,
  retry: (count, error) => ConnectError.from(error).code !== Code.Unauthenticated && count < 2,
});

/** The signed-in user. Only valid inside the authenticated app layout. */
export function useCurrentUser(): User {
  const { data } = useQuery(sessionQuery);
  if (data?.status !== "authenticated") {
    throw new Error("useCurrentUser used outside an authenticated route");
  }
  return data.user;
}

/** Refetches the session after login, setup, or logout. */
export function useRefreshSession() {
  const client = useQueryClient();
  return () => client.invalidateQueries({ queryKey: sessionQuery.queryKey });
}

export const ROLE_LABELS: Record<Role, string> = {
  [Role.UNSPECIFIED]: "Unknown role",
  [Role.VIEWER]: "Viewer",
  [Role.MODERATOR]: "Moderator",
  [Role.ADMIN]: "Administrator",
  [Role.OWNER]: "Owner",
};

/** Whether the signed-in user can change servers, the proxy, and settings. */
export function useCanManage(): boolean {
  const user = useCurrentUser();
  return user.role === Role.ADMIN || user.role === Role.OWNER;
}
