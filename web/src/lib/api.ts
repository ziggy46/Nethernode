import { Code, ConnectError, createClient, type Interceptor } from "@connectrpc/connect";
import { createConnectTransport } from "@connectrpc/connect-web";

import { AuthService } from "@/gen/nethernode/v1/auth_pb";

const CSRF_COOKIE = "nn_csrf";
const CSRF_HEADER = "x-csrf-token";
/** Set on 401s while first-run setup is pending. */
export const SETUP_REQUIRED_HEADER = "nn-setup-required";

function readCookie(name: string): string | undefined {
  for (const part of document.cookie.split(";")) {
    const [key, ...rest] = part.trim().split("=");
    if (key === name) return decodeURIComponent(rest.join("="));
  }
  return undefined;
}

/**
 * The server hands out the CSRF cookie on any response. If the page was
 * served by the Vite dev server, we may not have one yet, so fetch it.
 */
async function csrfToken(): Promise<string | undefined> {
  const existing = readCookie(CSRF_COOKIE);
  if (existing) return existing;
  await fetch("/healthz", { credentials: "same-origin" }).catch(() => undefined);
  return readCookie(CSRF_COOKIE);
}

/** Echoes the CSRF cookie in a header on requests with side effects. */
const csrf: Interceptor = (next) => async (req) => {
  if (req.requestMethod !== "GET") {
    const token = await csrfToken();
    if (token) req.header.set(CSRF_HEADER, token);
  }
  return next(req);
};

export const transport = createConnectTransport({
  baseUrl: "/api/rpc",
  // Side-effect-free RPCs (like Me) use GET, which needs no CSRF token.
  useHttpGet: true,
  interceptors: [csrf],
  fetch: (input, init) => fetch(input, { ...init, credentials: "same-origin" }),
});

export const authClient = createClient(AuthService, transport);

/** A user-facing message for an RPC failure. */
export function errorMessage(error: unknown): string {
  const err = ConnectError.from(error);
  switch (err.code) {
    case Code.Unavailable:
    case Code.Unknown:
      return "Can't reach NetherNode. Check that the server is running.";
    case Code.Internal:
      return "Something went wrong on the server. Try again.";
    default:
      return err.rawMessage || "Something went wrong. Try again.";
  }
}

export { Code, ConnectError };
