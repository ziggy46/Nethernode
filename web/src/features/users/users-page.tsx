import { createClient } from "@connectrpc/connect";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  Copy,
  Ellipsis,
  KeyRound,
  Lock,
  LogOut,
  ShieldCheck,
  ShieldOff,
  Trash2,
  TriangleAlert,
  UserCog,
  UserPlus,
} from "lucide-react";
import { useId, useState } from "react";
import { toast } from "sonner";

import { ConfirmDialog } from "@/components/confirm-dialog";
import { EmptyState, Footer, PageHeader } from "@/components/page";
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
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { ErrorAlert, Field, Select } from "@/components/ui/field";
import { Role, type User } from "@/gen/nethernode/v1/auth_pb";
import { UserService, type UserSummary } from "@/gen/nethernode/v1/users_pb";
import { errorMessage, transport } from "@/lib/api";
import { healthQuery } from "@/lib/health";
import { ROLE_LABELS, useCurrentUser } from "@/lib/session";
import { ago, dateTime } from "@/lib/time";
import { useDocumentTitle } from "@/lib/use-document-title";
import { useNow } from "@/lib/use-now";

import { usernameError } from "../auth/validation";

const userClient = createClient(UserService, transport);
const usersKey = ["users"] as const;

const ROLES: { role: Role; description: string }[] = [
  { role: Role.VIEWER, description: "Sees servers, players, and backups. Changes nothing." },
  { role: Role.MODERATOR, description: "Also uses the console and manages players." },
  { role: Role.ADMIN, description: "Also manages servers, files, backups, the proxy, and settings." },
  { role: Role.OWNER, description: "Full control, including users." },
];

type Action =
  | { kind: "role"; user: User }
  | { kind: "reset"; user: User }
  | { kind: "totp"; user: User }
  | { kind: "signout"; user: User }
  | { kind: "delete"; user: User };

/** Panel accounts, for owners. */
export function UsersPage() {
  useDocumentTitle("Users");
  const me = useCurrentUser();
  const health = useQuery(healthQuery);
  const owner = me.role === Role.OWNER;
  const users = useQuery({ queryKey: usersKey, queryFn: () => userClient.listUsers({}), enabled: owner });
  const [creating, setCreating] = useState(false);
  const [action, setAction] = useState<Action | null>(null);
  const [revealed, setRevealed] = useState<{ username: string; password: string } | null>(null);

  return (
    <>
      <PageHeader
        eyebrow="Access"
        title="Who can use this panel."
        description="Add people, choose what they can change, and help them back in."
        actions={
          owner && (
            <Button variant="primary" onClick={() => setCreating(true)}>
              <UserPlus />
              Add user
            </Button>
          )
        }
      />
      {!owner ? (
        <div className="rounded-lg border border-divider border-dashed bg-surface-subtle">
          <EmptyState icon={<Lock />} title="Users are managed by owners">
            Ask an owner if someone needs access or a password reset.
          </EmptyState>
        </div>
      ) : users.isError ? (
        <div className="rounded-lg border border-divider border-dashed bg-surface-subtle">
          <EmptyState
            icon={<TriangleAlert />}
            title="Couldn't load users"
            action={<Button onClick={() => void users.refetch()}>Try again</Button>}
          >
            {errorMessage(users.error)}
          </EmptyState>
        </div>
      ) : !users.data ? (
        <div className="h-48 animate-pulse rounded-lg bg-surface-subtle" role="status" aria-busy>
          <span className="sr-only">Loading users…</span>
        </div>
      ) : (
        <UsersTable rows={users.data.users} meId={me.id} onAction={setAction} />
      )}
      {creating && (
        <CreateDialog
          onClose={() => setCreating(false)}
          onCreated={(username, password) => {
            setCreating(false);
            setRevealed({ username, password });
          }}
        />
      )}
      {action && (
        <ActionDialog
          action={action}
          onClose={() => setAction(null)}
          onPassword={(password) => {
            setRevealed({ username: action.user.username, password });
            setAction(null);
          }}
        />
      )}
      {revealed && <PasswordReveal {...revealed} onClose={() => setRevealed(null)} />}
      <Footer version={health.data?.version} />
    </>
  );
}

function UsersTable({
  rows,
  meId,
  onAction,
}: {
  rows: UserSummary[];
  meId: string;
  onAction: (a: Action) => void;
}) {
  const now = useNow();
  return (
    <div className="overflow-x-auto rounded-lg border border-border">
      <table className="w-full min-w-[620px] text-label">
        <thead className="bg-surface-subtle text-left text-meta text-text-muted">
          <tr>
            {["User", "Role", "Two-factor", "Last sign-in", "Devices"].map((h) => (
              <th key={h} scope="col" className="border-divider border-b px-3 py-2.5 font-medium">
                {h}
              </th>
            ))}
            <th scope="col" className="border-divider border-b px-3 py-2.5">
              <span className="sr-only">Actions</span>
            </th>
          </tr>
        </thead>
        <tbody>
          {rows.map(({ user, activeSessions }) => {
            if (!user) return null;
            const self = user.id === meId;
            return (
              <tr key={user.id} className="border-divider border-b last:border-b-0">
                <td className="px-3 py-2.5">
                  <span className="font-semibold">{user.username}</span>
                  {self && <span className="ml-1.5 text-meta text-text-muted">(you)</span>}
                  {user.mustChangePassword && (
                    <span className="mt-0.5 block text-meta text-warning">Hasn't chosen a password yet</span>
                  )}
                </td>
                <td className="whitespace-nowrap px-3 py-2.5 text-text-secondary">
                  {ROLE_LABELS[user.role]}
                </td>
                <td className="px-3 py-2.5">
                  {user.totpEnabled ? (
                    <Badge tone="success" icon={<ShieldCheck className="size-3" />}>
                      On
                    </Badge>
                  ) : (
                    <Badge tone="muted">Off</Badge>
                  )}
                </td>
                <td className="whitespace-nowrap px-3 py-2.5 text-text-secondary">
                  {user.lastLoginAtMs ? (
                    <span title={dateTime(user.lastLoginAtMs)}>{ago(user.lastLoginAtMs, now)}</span>
                  ) : (
                    "Never"
                  )}
                </td>
                <td className="px-3 py-2.5 text-text-secondary tabular-nums">{String(activeSessions)}</td>
                <td className="px-3 py-2.5 text-right">
                  <DropdownMenu>
                    <DropdownMenuTrigger asChild>
                      <Button size="mini" variant="ghost" aria-label={`Actions for ${user.username}`}>
                        <Ellipsis />
                      </Button>
                    </DropdownMenuTrigger>
                    <DropdownMenuContent align="end">
                      <DropdownMenuItem onSelect={() => onAction({ kind: "role", user })}>
                        <UserCog />
                        Change role…
                      </DropdownMenuItem>
                      {!self && (
                        <DropdownMenuItem onSelect={() => onAction({ kind: "reset", user })}>
                          <KeyRound />
                          Reset password…
                        </DropdownMenuItem>
                      )}
                      {!self && user.totpEnabled && (
                        <DropdownMenuItem onSelect={() => onAction({ kind: "totp", user })}>
                          <ShieldOff />
                          Turn off two-factor…
                        </DropdownMenuItem>
                      )}
                      {!self && (
                        <DropdownMenuItem
                          disabled={activeSessions === 0n}
                          onSelect={() => onAction({ kind: "signout", user })}
                        >
                          <LogOut />
                          Sign out everywhere…
                        </DropdownMenuItem>
                      )}
                      {!self && (
                        <>
                          <DropdownMenuSeparator />
                          <DropdownMenuItem
                            className="text-danger"
                            onSelect={() => onAction({ kind: "delete", user })}
                          >
                            <Trash2 />
                            Delete…
                          </DropdownMenuItem>
                        </>
                      )}
                    </DropdownMenuContent>
                  </DropdownMenu>
                </td>
              </tr>
            );
          })}
        </tbody>
      </table>
    </div>
  );
}

function RoleSelect({ id, value, onChange }: { id: string; value: Role; onChange: (r: Role) => void }) {
  return (
    <div className="mb-[17px]">
      <label htmlFor={id} className="mb-[7px] block text-label text-text-secondary">
        Role
      </label>
      <Select id={id} value={value} onChange={(e) => onChange(Number(e.target.value) as Role)}>
        {ROLES.map((r) => (
          <option key={r.role} value={r.role}>
            {ROLE_LABELS[r.role]}
          </option>
        ))}
      </Select>
      <p className="mt-1.5 text-meta text-text-muted">{ROLES.find((r) => r.role === value)?.description}</p>
    </div>
  );
}

function CreateDialog({
  onClose,
  onCreated,
}: {
  onClose: () => void;
  onCreated: (username: string, password: string) => void;
}) {
  const client = useQueryClient();
  const roleId = useId();
  const [username, setUsername] = useState("");
  const [role, setRole] = useState(Role.VIEWER);
  const [touched, setTouched] = useState(false);
  const create = useMutation({
    mutationFn: () => userClient.createUser({ username: username.trim(), role }),
    onSuccess: ({ user, password }) => {
      void client.invalidateQueries({ queryKey: usersKey });
      onCreated(user?.username ?? username.trim(), password);
    },
  });
  const nameError = touched ? usernameError(username) : undefined;
  return (
    <Dialog open onOpenChange={(o) => !o && onClose()}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>Add a user</DialogTitle>
          <DialogDescription>
            They get a one-time password to sign in with, then choose their own.
          </DialogDescription>
        </DialogHeader>
        <form
          onSubmit={(e) => {
            e.preventDefault();
            setTouched(true);
            if (!usernameError(username)) create.mutate();
          }}
          noValidate
        >
          {create.isError && <ErrorAlert>{errorMessage(create.error)}</ErrorAlert>}
          <Field
            label="Username"
            autoComplete="off"
            autoCapitalize="none"
            spellCheck={false}
            autoFocus
            error={nameError}
            value={username}
            onChange={(e) => setUsername(e.target.value)}
          />
          <RoleSelect id={roleId} value={role} onChange={setRole} />
          <DialogFooter>
            <Button onClick={onClose}>Cancel</Button>
            <Button type="submit" variant="primary" disabled={create.isPending}>
              <UserPlus />
              {create.isPending ? "Adding…" : "Add user"}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}

function ActionDialog({
  action,
  onClose,
  onPassword,
}: {
  action: Action;
  onClose: () => void;
  onPassword: (password: string) => void;
}) {
  const client = useQueryClient();
  const roleId = useId();
  const { user } = action;
  const [role, setRole] = useState(user.role);
  const run = useMutation({
    mutationFn: async (): Promise<string | undefined> => {
      switch (action.kind) {
        case "role":
          await userClient.setUserRole({ id: user.id, role });
          return;
        case "reset":
          return (await userClient.resetUserPassword({ id: user.id })).password;
        case "totp":
          await userClient.resetUserTotp({ id: user.id });
          return;
        case "signout":
          await userClient.revokeUserSessions({ id: user.id });
          return;
        case "delete":
          await userClient.deleteUser({ id: user.id });
          return;
      }
    },
    onSuccess: (password) => {
      void client.invalidateQueries({ queryKey: usersKey });
      if (password) {
        onPassword(password);
        return;
      }
      toast.success(
        {
          role: `${user.username} is now ${ROLE_LABELS[role].toLowerCase()}.`,
          reset: "",
          totp: `Two-factor sign-in is off for ${user.username}.`,
          signout: `${user.username} was signed out everywhere.`,
          delete: `${user.username} was deleted.`,
        }[action.kind],
      );
      onClose();
    },
  });
  const error = run.error ? errorMessage(run.error) : undefined;

  if (action.kind === "role") {
    return (
      <Dialog open onOpenChange={(o) => !o && onClose()}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>Change {user.username}'s role</DialogTitle>
            <DialogDescription>It takes effect on their next click; they stay signed in.</DialogDescription>
          </DialogHeader>
          {error && <ErrorAlert>{error}</ErrorAlert>}
          <RoleSelect id={roleId} value={role} onChange={setRole} />
          <DialogFooter>
            <Button onClick={onClose}>Cancel</Button>
            <Button
              variant="primary"
              disabled={run.isPending || role === user.role}
              onClick={() => run.mutate()}
            >
              {run.isPending ? "Saving…" : "Change role"}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    );
  }

  const copy = {
    reset: {
      title: `Reset ${user.username}'s password?`,
      description:
        "They're signed out everywhere and get a one-time password, which you'll see next. They choose a new password when they sign in.",
      confirm: "Reset password",
    },
    totp: {
      title: `Turn off two-factor sign-in for ${user.username}?`,
      description:
        "For a lost phone. Their password alone will sign them in until they set it up again. Make sure the request really came from them.",
      confirm: "Turn off",
    },
    signout: {
      title: `Sign ${user.username} out everywhere?`,
      description: "Every device they're signed in on is signed out. Their password stays the same.",
      confirm: "Sign out",
    },
    delete: {
      title: `Delete ${user.username}?`,
      description: "They lose access right away. Their past actions stay in the audit log.",
      confirm: "Delete user",
    },
  }[action.kind];
  return (
    <ConfirmDialog
      open
      onOpenChange={(o) => !o && onClose()}
      title={copy.title}
      description={copy.description}
      confirmLabel={copy.confirm}
      pending={run.isPending}
      error={error}
      onConfirm={() => run.mutate()}
    />
  );
}

/** A one-time password, shown once. */
function PasswordReveal({
  username,
  password,
  onClose,
}: {
  username: string;
  password: string;
  onClose: () => void;
}) {
  const copy = async () => {
    try {
      await navigator.clipboard.writeText(password);
      toast.success("Password copied.");
    } catch {
      toast.error("Couldn't copy. Select the password and copy it instead.");
    }
  };
  return (
    <Dialog open onOpenChange={(o) => !o && onClose()}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>One-time password for {username}</DialogTitle>
          <DialogDescription>
            Share it privately. They sign in with it once and then choose their own. It won't be shown again.
          </DialogDescription>
        </DialogHeader>
        <p className="select-all break-all rounded-md border border-divider bg-surface px-3.5 py-3 text-center font-mono text-section">
          {password}
        </p>
        <DialogFooter>
          <Button onClick={() => void copy()}>
            <Copy />
            Copy
          </Button>
          <Button variant="primary" onClick={onClose}>
            Done
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
