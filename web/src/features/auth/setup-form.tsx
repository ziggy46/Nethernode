import { useMutation, useQueryClient } from "@tanstack/react-query";
import { Link, useNavigate } from "@tanstack/react-router";
import { ShieldCheck } from "lucide-react";
import { type FormEvent, useState } from "react";

import { Button } from "@/components/ui/button";
import { ErrorAlert, Field } from "@/components/ui/field";
import { authClient, Code, ConnectError, errorMessage } from "@/lib/api";
import { type Session, sessionQuery } from "@/lib/session";

import { MIN_PASSWORD, passwordError, usernameError } from "./validation";

export function SetupForm() {
  const queryClient = useQueryClient();
  const navigate = useNavigate();
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [confirm, setConfirm] = useState("");
  const [submitted, setSubmitted] = useState(false);

  const errors = {
    username: usernameError(username),
    password: passwordError(password),
    confirm: confirm === password ? undefined : "Passwords don't match.",
  };
  const valid = !errors.username && !errors.password && !errors.confirm;

  const setup = useMutation({
    mutationFn: () => authClient.setup({ username: username.trim(), password }),
    onSuccess: async ({ user }) => {
      if (!user) return;
      const session: Session = { status: "authenticated", user };
      queryClient.setQueryData(sessionQuery.queryKey, session);
      // Next: the optional domain step.
      await navigate({ to: "/welcome" });
    },
  });

  const onSubmit = (e: FormEvent) => {
    e.preventDefault();
    setSubmitted(true);
    if (valid) setup.mutate();
  };

  const alreadyDone = setup.isError && ConnectError.from(setup.error).code === Code.FailedPrecondition;

  return (
    <form onSubmit={onSubmit} noValidate>
      {setup.isError &&
        (alreadyDone ? (
          <ErrorAlert>
            Setup has already been completed.{" "}
            <Link to="/login" className="underline underline-offset-2">
              Sign in instead
            </Link>
            .
          </ErrorAlert>
        ) : (
          <ErrorAlert>{errorMessage(setup.error)}</ErrorAlert>
        ))}
      <Field
        label="Username"
        name="username"
        autoComplete="username"
        autoCapitalize="none"
        spellCheck={false}
        required
        autoFocus
        hint="3–32 letters, numbers, '_', '.', or '-'."
        error={submitted ? errors.username : undefined}
        value={username}
        onChange={(e) => setUsername(e.target.value)}
      />
      <Field
        label="Password"
        name="password"
        type="password"
        autoComplete="new-password"
        required
        hint={`At least ${MIN_PASSWORD} characters. A passphrase works well.`}
        error={submitted ? errors.password : undefined}
        value={password}
        onChange={(e) => setPassword(e.target.value)}
      />
      <Field
        label="Confirm password"
        name="confirm"
        type="password"
        autoComplete="new-password"
        required
        error={submitted ? errors.confirm : undefined}
        value={confirm}
        onChange={(e) => setConfirm(e.target.value)}
      />
      <Button type="submit" variant="primary" size="lg" className="mt-2 w-full" disabled={setup.isPending}>
        <ShieldCheck />
        {setup.isPending ? "Creating account…" : "Create owner account"}
      </Button>
    </form>
  );
}
