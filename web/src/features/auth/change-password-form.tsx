import { useMutation, useQueryClient } from "@tanstack/react-query";
import { KeyRound } from "lucide-react";
import { type FormEvent, useState } from "react";

import { Button } from "@/components/ui/button";
import { ErrorAlert, Field } from "@/components/ui/field";
import { errorMessage } from "@/lib/api";
import { type Session, sessionQuery } from "@/lib/session";

import { accountClient } from "../account/api";
import { passwordError } from "./validation";

/** Current password, a new one, and the new one again. */
export function ChangePasswordForm({
  currentLabel = "Current password",
  onDone,
  children,
}: {
  currentLabel?: string;
  onDone?: () => void;
  children?: React.ReactNode;
}) {
  const client = useQueryClient();
  const [current, setCurrent] = useState("");
  const [next, setNext] = useState("");
  const [again, setAgain] = useState("");
  const [touched, setTouched] = useState(false);
  const change = useMutation({
    mutationFn: () => accountClient.changePassword({ currentPassword: current, newPassword: next }),
    onSuccess: ({ user }) => {
      if (user) {
        const session: Session = { status: "authenticated", user };
        client.setQueryData(sessionQuery.queryKey, session);
      }
      onDone?.();
    },
  });
  const nextError = touched ? passwordError(next) : undefined;
  const againError = touched && again !== next ? "The passwords don't match." : undefined;

  const onSubmit = (e: FormEvent) => {
    e.preventDefault();
    setTouched(true);
    if (!current || passwordError(next) || again !== next) return;
    change.mutate();
  };

  return (
    <form onSubmit={onSubmit} noValidate>
      {change.isError && <ErrorAlert>{errorMessage(change.error)}</ErrorAlert>}
      <Field
        label={currentLabel}
        type="password"
        autoComplete="current-password"
        required
        autoFocus
        value={current}
        onChange={(e) => setCurrent(e.target.value)}
      />
      <Field
        label="New password"
        type="password"
        autoComplete="new-password"
        hint="At least 10 characters. A few unrelated words work well."
        error={nextError}
        required
        value={next}
        onChange={(e) => setNext(e.target.value)}
      />
      <Field
        label="New password again"
        type="password"
        autoComplete="new-password"
        error={againError}
        required
        value={again}
        onChange={(e) => setAgain(e.target.value)}
      />
      <p className="mb-[17px] text-meta text-text-muted">Your other signed-in devices will be signed out.</p>
      <div className="flex flex-wrap items-center justify-end gap-2">
        {children}
        <Button type="submit" variant="primary" disabled={change.isPending}>
          <KeyRound />
          {change.isPending ? "Saving…" : "Change password"}
        </Button>
      </div>
    </form>
  );
}
