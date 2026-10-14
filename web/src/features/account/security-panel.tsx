import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Copy, KeyRound, LaptopMinimal, ShieldCheck } from "lucide-react";
import { useState } from "react";
import { toast } from "sonner";

import { ConfirmDialog } from "@/components/confirm-dialog";
import { Panel, PanelHeader } from "@/components/page";
import { Badge } from "@/components/status-badge";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { ErrorAlert, Field } from "@/components/ui/field";
import { errorMessage } from "@/lib/api";
import { sessionQuery } from "@/lib/session";
import { ago } from "@/lib/time";
import { useNow } from "@/lib/use-now";

import { ChangePasswordForm } from "../auth/change-password-form";
import { describeAgent } from "./agent";
import { accountClient } from "./api";

const securityKey = ["security"] as const;

/** Password, two-factor sign-in, and signed-in devices. */
export default function SecurityPanel() {
  const client = useQueryClient();
  const now = useNow();
  const security = useQuery({ queryKey: securityKey, queryFn: () => accountClient.getSecurity({}) });
  const [dialog, setDialog] = useState<"password" | "enroll" | "disable" | null>(null);
  const revoke = useMutation({
    mutationFn: (id: string) => accountClient.revokeSession({ id }),
    onSuccess: () => {
      toast.success("Signed out that device.");
      void client.invalidateQueries({ queryKey: securityKey });
    },
    onError: (e) => toast.error(errorMessage(e)),
  });
  const refresh = () => {
    void client.invalidateQueries({ queryKey: securityKey });
    void client.invalidateQueries({ queryKey: sessionQuery.queryKey });
  };
  const data = security.data;

  return (
    <Panel>
      <PanelHeader title="Sign-in and security" icon={<ShieldCheck />} />
      <Row title="Password" description="Changing it signs out your other devices.">
        <Button variant="subtle" onClick={() => setDialog("password")}>
          <KeyRound />
          Change…
        </Button>
      </Row>
      <Row
        title="Two-factor sign-in"
        description={
          !data
            ? "Checking…"
            : data.totpEnabled
              ? `On. ${data.recoveryCodesLeft} of 10 recovery codes left.`
              : data.totpAvailable
                ? "Ask for a code from an authenticator app as well as your password."
                : "Unavailable: NetherNode couldn't load its data key."
        }
      >
        {data?.totpEnabled ? (
          <div className="flex items-center gap-2">
            <Badge tone="success" icon={<ShieldCheck className="size-3" />}>
              On
            </Badge>
            <Button variant="subtle" onClick={() => setDialog("disable")}>
              Turn off…
            </Button>
          </div>
        ) : (
          <Button variant="subtle" disabled={!data?.totpAvailable} onClick={() => setDialog("enroll")}>
            <ShieldCheck />
            Set up…
          </Button>
        )}
      </Row>
      <div className="pt-[15px]">
        <p className="font-[550] text-label">Signed-in devices</p>
        {security.isError ? (
          <p className="mt-2 text-meta text-danger">{errorMessage(security.error)}</p>
        ) : !data ? (
          <div className="mt-2 h-12 animate-pulse rounded-md bg-surface" />
        ) : (
          <ul className="mt-2 divide-y divide-divider">
            {data.sessions.map((s) => (
              <li key={s.id} className="flex items-center gap-3 py-2.5">
                <LaptopMinimal className="size-4 shrink-0 text-text-muted" aria-hidden />
                <div className="min-w-0 flex-1">
                  <p className="truncate text-label">
                    {describeAgent(s.userAgent)}
                    {s.current && <span className="ml-1.5 text-meta text-success">This device</span>}
                  </p>
                  <p className="truncate text-meta text-text-muted">
                    {s.ip || "Unknown address"} · signed in {ago(s.createdAtMs, now)}
                  </p>
                </div>
                {!s.current && (
                  <Button
                    size="mini"
                    variant="ghost"
                    disabled={revoke.isPending}
                    onClick={() => revoke.mutate(s.id)}
                  >
                    Sign out
                  </Button>
                )}
              </li>
            ))}
          </ul>
        )}
      </div>

      <Dialog open={dialog === "password"} onOpenChange={(o) => !o && setDialog(null)}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>Change your password</DialogTitle>
            <DialogDescription>Use one you don't use anywhere else.</DialogDescription>
          </DialogHeader>
          <ChangePasswordForm
            onDone={() => {
              toast.success("Password changed.");
              setDialog(null);
              refresh();
            }}
          />
        </DialogContent>
      </Dialog>
      {dialog === "enroll" && (
        <EnrollDialog
          onClose={() => {
            setDialog(null);
            refresh();
          }}
        />
      )}
      {dialog === "disable" && (
        <DisableDialog
          onClose={() => {
            setDialog(null);
            refresh();
          }}
        />
      )}
    </Panel>
  );
}

function Row({
  title,
  description,
  children,
}: {
  title: string;
  description: string;
  children: React.ReactNode;
}) {
  return (
    <div className="flex items-center justify-between gap-5 border-divider border-b py-[15px] first-of-type:pt-0 below-sm:flex-wrap">
      <div className="min-w-0">
        <p className="font-[550] text-label">{title}</p>
        <p className="mt-[3px] text-meta text-text-muted">{description}</p>
      </div>
      <div className="shrink-0">{children}</div>
    </div>
  );
}

/** Scan, confirm with a code, then save the recovery codes. */
function EnrollDialog({ onClose }: { onClose: () => void }) {
  const setup = useQuery({
    queryKey: ["security", "enroll"],
    queryFn: () => accountClient.beginTotpSetup({}),
    staleTime: Number.POSITIVE_INFINITY,
    gcTime: 0,
    retry: false,
  });
  const [code, setCode] = useState("");
  const [codes, setCodes] = useState<string[] | null>(null);
  const confirm = useMutation({
    mutationFn: () => accountClient.confirmTotpSetup({ code: code.trim() }),
    onSuccess: ({ recoveryCodes }) => setCodes(recoveryCodes),
    onError: () => setCode(""),
  });
  const copy = async (text: string, what: string) => {
    try {
      await navigator.clipboard.writeText(text);
      toast.success(`${what} copied.`);
    } catch {
      toast.error("Couldn't copy. Select the text and copy it instead.");
    }
  };

  return (
    <Dialog open onOpenChange={(o) => !o && onClose()}>
      <DialogContent className="max-w-[460px]">
        {codes ? (
          <>
            <DialogHeader>
              <DialogTitle>Save your recovery codes</DialogTitle>
              <DialogDescription>
                Two-factor sign-in is on. If you lose your phone, each of these codes signs you in once. Keep
                them somewhere safe, like a password manager. They won't be shown again.
              </DialogDescription>
            </DialogHeader>
            <ul className="grid grid-cols-2 gap-x-4 gap-y-1.5 rounded-md border border-divider bg-surface p-3.5 font-mono text-label">
              {codes.map((c) => (
                <li key={c}>{c}</li>
              ))}
            </ul>
            <DialogFooter>
              <Button onClick={() => void copy(codes.join("\n"), "Recovery codes")}>
                <Copy />
                Copy
              </Button>
              <Button variant="primary" onClick={onClose}>
                I've saved them
              </Button>
            </DialogFooter>
          </>
        ) : (
          <>
            <DialogHeader>
              <DialogTitle>Set up two-factor sign-in</DialogTitle>
              <DialogDescription>
                Scan this with an authenticator app (such as 1Password, Google Authenticator, or Aegis), then
                enter the code it shows.
              </DialogDescription>
            </DialogHeader>
            {setup.isError ? (
              <ErrorAlert>{errorMessage(setup.error)}</ErrorAlert>
            ) : !setup.data ? (
              <div
                className="mx-auto size-[200px] animate-pulse rounded-md bg-surface"
                role="status"
                aria-busy
              >
                <span className="sr-only">Preparing…</span>
              </div>
            ) : (
              <>
                <img
                  src={`data:image/svg+xml;charset=utf-8,${encodeURIComponent(setup.data.qrSvg)}`}
                  alt="QR code to scan with your authenticator app"
                  width={200}
                  height={200}
                  className="mx-auto rounded-md"
                />
                <p className="mt-3 text-center text-meta text-text-muted">
                  Can't scan it? Enter this key instead:
                </p>
                <button
                  type="button"
                  className="mx-auto mt-1 mb-[17px] flex items-center gap-1.5 break-all rounded-sm px-1.5 py-0.5 font-mono text-label hover:bg-surface-elevated"
                  onClick={() => void copy(setup.data.secret, "Key")}
                >
                  {setup.data.secret.replace(/(.{4})/g, "$1 ").trim()}
                  <Copy className="size-3.5 shrink-0 text-text-muted" aria-hidden />
                </button>
                <form
                  onSubmit={(e) => {
                    e.preventDefault();
                    if (code.trim()) confirm.mutate();
                  }}
                >
                  {confirm.isError && <ErrorAlert>{errorMessage(confirm.error)}</ErrorAlert>}
                  <Field
                    label="Code from the app"
                    autoComplete="one-time-code"
                    inputMode="numeric"
                    maxLength={7}
                    placeholder="123 456"
                    inputClassName="font-mono"
                    autoFocus
                    value={code}
                    onChange={(e) => setCode(e.target.value)}
                  />
                  <DialogFooter>
                    <Button onClick={onClose}>Cancel</Button>
                    <Button type="submit" variant="primary" disabled={confirm.isPending || !code.trim()}>
                      {confirm.isPending ? "Checking…" : "Turn on"}
                    </Button>
                  </DialogFooter>
                </form>
              </>
            )}
          </>
        )}
      </DialogContent>
    </Dialog>
  );
}

function DisableDialog({ onClose }: { onClose: () => void }) {
  const [password, setPassword] = useState("");
  const disable = useMutation({
    mutationFn: () => accountClient.disableTotp({ password }),
    onSuccess: () => {
      toast.success("Two-factor sign-in is off.");
      onClose();
    },
    onError: () => setPassword(""),
  });
  return (
    <ConfirmDialog
      open
      onOpenChange={(o) => !o && onClose()}
      title="Turn off two-factor sign-in?"
      description="Your password alone will sign you in again, and your recovery codes stop working."
      confirmLabel="Turn off"
      pending={disable.isPending}
      error={disable.error ? errorMessage(disable.error) : undefined}
      onConfirm={() => disable.mutate()}
    >
      <Field
        label="Your password"
        type="password"
        autoComplete="current-password"
        autoFocus
        value={password}
        onChange={(e) => setPassword(e.target.value)}
      />
    </ConfirmDialog>
  );
}
