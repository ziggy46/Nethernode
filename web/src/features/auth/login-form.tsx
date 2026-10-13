import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useNavigate } from "@tanstack/react-router";
import { LogIn } from "lucide-react";
import { type FormEvent, useState } from "react";

import { Button } from "@/components/ui/button";
import { ErrorAlert, Field } from "@/components/ui/field";
import { authClient, Code, ConnectError, errorMessage } from "@/lib/api";
import { type Session, sessionQuery } from "@/lib/session";

function loginErrorMessage(error: unknown): string {
  const err = ConnectError.from(error);
  if (err.code === Code.Unauthenticated) return "Incorrect username or password.";
  if (err.code === Code.ResourceExhausted) {
    const seconds = Number(err.metadata.get("retry-after"));
    return Number.isFinite(seconds) && seconds > 0
      ? `Too many attempts. Try again in ${seconds} second${seconds === 1 ? "" : "s"}.`
      : "Too many attempts. Wait a moment and try again.";
  }
  return errorMessage(error);
}

export function LoginForm({ redirectTo }: { redirectTo?: string | undefined }) {
  const queryClient = useQueryClient();
  const navigate = useNavigate();
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");

  const login = useMutation({
    mutationFn: () => authClient.login({ username: username.trim(), password }),
    onSuccess: async ({ user }) => {
      if (!user) return;
      const session: Session = { status: "authenticated", user };
      queryClient.setQueryData(sessionQuery.queryKey, session);
      // Only follow same-app paths, never absolute URLs.
      const target = redirectTo?.startsWith("/") && !redirectTo.startsWith("//") ? redirectTo : "/";
      await navigate({ to: target });
    },
    onError: () => setPassword(""),
  });

  const onSubmit = (e: FormEvent) => {
    e.preventDefault();
    if (!username.trim() || !password) return;
    login.mutate();
  };

  return (
    <form onSubmit={onSubmit} noValidate>
      {login.isError && <ErrorAlert>{loginErrorMessage(login.error)}</ErrorAlert>}
      <Field
        label="Username"
        name="username"
        autoComplete="username"
        autoCapitalize="none"
        spellCheck={false}
        required
        autoFocus
        value={username}
        onChange={(e) => setUsername(e.target.value)}
      />
      <Field
        label="Password"
        name="password"
        type="password"
        autoComplete="current-password"
        required
        value={password}
        onChange={(e) => setPassword(e.target.value)}
      />
      <Button
        type="submit"
        variant="primary"
        size="lg"
        className="mt-2 w-full"
        disabled={login.isPending || !username.trim() || !password}
      >
        <LogIn />
        {login.isPending ? "Signing in…" : "Sign in"}
      </Button>
    </form>
  );
}
