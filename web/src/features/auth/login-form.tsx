import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useNavigate } from "@tanstack/react-router";
import { KeyRound, LogIn } from "lucide-react";
import { type FormEvent, useState } from "react";

import { Button } from "@/components/ui/button";
import { ErrorAlert, Field } from "@/components/ui/field";
import type { User } from "@/gen/nethernode/v1/auth_pb";
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
  const [challenge, setChallenge] = useState<string | null>(null);

  const signedIn = async (user: User) => {
    const session: Session = { status: "authenticated", user };
    queryClient.setQueryData(sessionQuery.queryKey, session);
    // Only follow same-app paths, never absolute URLs.
    const target = redirectTo?.startsWith("/") && !redirectTo.startsWith("//") ? redirectTo : "/";
    await navigate({ to: target });
  };

  const login = useMutation({
    mutationFn: () => authClient.login({ username: username.trim(), password }),
    onSuccess: async ({ user, secondFactorChallenge }) => {
      setPassword("");
      if (secondFactorChallenge) setChallenge(secondFactorChallenge);
      else if (user) await signedIn(user);
    },
    onError: () => setPassword(""),
  });

  const onSubmit = (e: FormEvent) => {
    e.preventDefault();
    if (!username.trim() || !password) return;
    login.mutate();
  };

  if (challenge) {
    return <SecondFactor challenge={challenge} onDone={signedIn} onRestart={() => setChallenge(null)} />;
  }

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

/** The second sign-in step: an authenticator or recovery code. */
function SecondFactor({
  challenge,
  onDone,
  onRestart,
}: {
  challenge: string;
  onDone: (user: User) => Promise<void>;
  onRestart: () => void;
}) {
  const [code, setCode] = useState("");
  const [recovery, setRecovery] = useState(false);
  const finish = useMutation({
    mutationFn: () => authClient.finishSignIn({ challenge, code: code.trim() }),
    onSuccess: async ({ user }) => {
      if (user) await onDone(user);
    },
    onError: () => setCode(""),
  });
  const expired = finish.isError && errorMessage(finish.error).includes("expired");

  return (
    <form
      onSubmit={(e) => {
        e.preventDefault();
        if (code.trim()) finish.mutate();
      }}
      noValidate
    >
      <p className="mb-[17px] text-label text-text-secondary">Two-factor sign-in is on for this account.</p>
      {finish.isError && <ErrorAlert>{loginErrorMessage(finish.error)}</ErrorAlert>}
      {recovery ? (
        <Field
          key="recovery"
          label="Recovery code"
          name="recovery-code"
          autoComplete="off"
          autoCapitalize="none"
          spellCheck={false}
          placeholder="xxxxx-xxxxx"
          hint="Each recovery code works once."
          className="font-mono"
          required
          autoFocus
          value={code}
          onChange={(e) => setCode(e.target.value)}
        />
      ) : (
        <Field
          key="totp"
          label="Authenticator code"
          name="one-time-code"
          autoComplete="one-time-code"
          inputMode="numeric"
          pattern="[0-9 ]*"
          maxLength={7}
          placeholder="123 456"
          hint="The 6-digit code from your authenticator app."
          className="font-mono"
          required
          autoFocus
          value={code}
          onChange={(e) => setCode(e.target.value)}
        />
      )}
      {expired ? (
        <Button type="button" variant="primary" size="lg" className="mt-2 w-full" onClick={onRestart}>
          Start again
        </Button>
      ) : (
        <Button
          type="submit"
          variant="primary"
          size="lg"
          className="mt-2 w-full"
          disabled={finish.isPending || !code.trim()}
        >
          <KeyRound />
          {finish.isPending ? "Checking…" : "Verify"}
        </Button>
      )}
      <button
        type="button"
        className="mt-4 w-full text-center text-label text-text-muted underline-offset-2 hover:text-text-primary hover:underline"
        onClick={() => {
          setRecovery((r) => !r);
          setCode("");
          finish.reset();
        }}
      >
        {recovery ? "Use your authenticator app instead" : "Lost your phone? Use a recovery code"}
      </button>
    </form>
  );
}
