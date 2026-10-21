import { createClient } from "@connectrpc/connect";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  Blocks,
  CircleAlert,
  CircleCheck,
  CircleDashed,
  CirclePause,
  Globe,
  Info,
  Play,
  Plus,
  ScrollText,
  Settings2,
  Square,
  Trash2,
} from "lucide-react";
import { useId, useState } from "react";
import { toast } from "sonner";

import { ConfirmDialog } from "@/components/confirm-dialog";
import { EmptyState } from "@/components/page";
import { Badge } from "@/components/status-badge";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { ErrorAlert, Field, Select } from "@/components/ui/field";
import { Switch } from "@/components/ui/switch";
import {
  type Module,
  ModuleService,
  type ModuleSetting,
  type ModuleTemplate,
} from "@/gen/nethernode/v1/modules_pb";
import type { Server } from "@/gen/nethernode/v1/servers_pb";
import { errorMessage, transport } from "@/lib/api";

import { routesQuery } from "../proxy/queries";
import { serverAddresses, siteAddresses, siteError } from "./site";

const client = createClient(ModuleService, transport);
const modulesKey = (serverId: string) => ["modules", serverId] as const;
const templatesKey = ["module-templates"] as const;

const LIFECYCLES = [
  {
    value: "with-server",
    label: "With the server",
    hint: "Runs while the server runs; stops when it stops or sleeps.",
  },
  { value: "always", label: "All the time", hint: "Keeps running whatever the server is doing." },
  { value: "manual", label: "Only when started", hint: "Starts and stops only when you say." },
];

function lifecycleLabel(value: string): string {
  return LIFECYCLES.find((l) => l.value === value)?.label ?? value;
}

function StatusBadge({ module }: { module: Module }) {
  switch (module.status) {
    case "running":
      return (
        <Badge tone="success" icon={<CircleCheck className="size-3" />}>
          Running
        </Badge>
      );
    case "failed":
      return (
        <Badge tone="danger" icon={<CircleAlert className="size-3" />}>
          Failed
        </Badge>
      );
    case "stopped":
      return (
        <Badge tone="muted" icon={<CirclePause className="size-3" />}>
          Stopped
        </Badge>
      );
    default:
      return (
        <Badge tone="muted" icon={<CircleDashed className="size-3" />}>
          Not started
        </Badge>
      );
  }
}

/** The Modules section of a server's page. */
export default function ModulesTab({ server }: { server: Server }) {
  const queryClient = useQueryClient();
  const list = useQuery({
    queryKey: modulesKey(server.id),
    queryFn: () => client.listModules({ serverId: server.id }),
    refetchInterval: 5000,
  });
  const templates = useQuery({ queryKey: templatesKey, queryFn: () => client.listModuleTemplates({}) });
  const [adding, setAdding] = useState<ModuleTemplate | null>(null);
  const [editing, setEditing] = useState<Module | null>(null);
  const [logs, setLogs] = useState<Module | null>(null);
  const [site, setSite] = useState<Module | null>(null);
  const [removing, setRemoving] = useState<Module | null>(null);
  const [deleteFiles, setDeleteFiles] = useState(false);
  const refresh = () => queryClient.invalidateQueries({ queryKey: modulesKey(server.id) });
  const act = useMutation({
    mutationFn: async ({ work }: { work: () => Promise<unknown>; done?: string }) => work(),
    onSuccess: (_, { done }) => done && toast.success(done),
    onError: (e) => toast.error(errorMessage(e)),
    onSettled: refresh,
  });

  if (list.isPending) return <div className="h-64 animate-pulse rounded-lg bg-surface-subtle" aria-busy />;
  if (list.isError) {
    return (
      <EmptyState icon={<CircleAlert />} title="Couldn't list modules">
        {errorMessage(list.error)}
      </EmptyState>
    );
  }
  const modules = list.data.modules;
  const unavailable = list.data.unavailable;
  const available = templates.data?.templates ?? [];
  const templateOf = (m: Module) => available.find((t) => t.id === m.template);

  return (
    <div>
      <div className="mb-4 flex flex-wrap items-start justify-between gap-3">
        <p className="max-w-prose text-label text-text-secondary">
          Modules run next to this server in their own containers, with their files in its{" "}
          <span className="font-mono">modules</span> folder.
        </p>
      </div>
      {unavailable && <ErrorAlert className="mb-4">{unavailable}</ErrorAlert>}

      {modules.length > 0 && (
        <ul className="mb-6 divide-y divide-divider rounded-lg border border-border">
          {modules.map((m) => {
            const template = templateOf(m);
            const running = m.status === "running";
            const busy = act.isPending && act.variables?.work !== undefined;
            return (
              <li key={m.id} className="flex flex-wrap items-center gap-3 px-3.5 py-3">
                <Blocks className="size-5 shrink-0 text-text-muted" aria-hidden />
                <div className="min-w-0 flex-1">
                  <p className="flex flex-wrap items-center gap-x-2 gap-y-1">
                    <span className="font-medium text-label">{template?.name ?? m.template}</span>
                    {m.name !== m.template && (
                      <span className="font-mono text-meta text-text-muted">{m.name}</span>
                    )}
                    <StatusBadge module={m} />
                  </p>
                  <p className="text-meta text-text-muted">
                    {lifecycleLabel(m.lifecycle)} · {m.memoryMb} MB
                    {template?.version ? ` · ${template.name} ${template.version}` : ""}
                  </p>
                  {m.webAddress && (
                    <p className="text-meta text-text-muted">
                      Web page:{" "}
                      <span className="select-all font-mono text-text-secondary">{m.webAddress}</span>, on the
                      machine NetherNode runs on
                    </p>
                  )}
                  {m.webAddress && (
                    <p className="flex flex-wrap items-center gap-x-1.5 text-meta text-text-muted">
                      <Globe className="size-3.5" aria-hidden />
                      {m.webPublic && m.siteAddresses.length > 0 ? (
                        <>
                          Open to anyone at{" "}
                          {m.siteAddresses.map((a, i) => (
                            <span key={a}>
                              {i > 0 && ", "}
                              <a
                                href={`https://${a}`}
                                target="_blank"
                                rel="noreferrer noopener"
                                className="font-mono text-text-secondary underline underline-offset-2"
                              >
                                {a}
                              </a>
                            </span>
                          ))}
                        </>
                      ) : m.webPublic ? (
                        "Open to anyone, but the server has no address for the prefix to go in front of."
                      ) : (
                        "Not on an address of its own."
                      )}
                      <button
                        type="button"
                        className="text-text-secondary underline underline-offset-2"
                        onClick={() => setSite(m)}
                      >
                        Change
                      </button>
                    </p>
                  )}
                  {m.message && <p className="text-meta text-danger">{m.message}</p>}
                </div>
                <div className="flex gap-1.5">
                  {running ? (
                    <Button
                      size="mini"
                      disabled={busy}
                      onClick={() =>
                        act.mutate({
                          work: () => client.stopModule({ id: m.id }),
                          done: `${m.name} stopped.`,
                        })
                      }
                    >
                      <Square />
                      Stop
                    </Button>
                  ) : (
                    <Button
                      size="mini"
                      disabled={busy || Boolean(unavailable)}
                      onClick={() =>
                        act.mutate({
                          work: () => client.startModule({ id: m.id }),
                          done: `${m.name} started.`,
                        })
                      }
                    >
                      <Play />
                      {busy ? "Starting…" : "Start"}
                    </Button>
                  )}
                  <Button
                    size="mini"
                    variant="ghost"
                    aria-label={`Log of ${m.name}`}
                    onClick={() => setLogs(m)}
                  >
                    <ScrollText />
                  </Button>
                  <Button
                    size="mini"
                    variant="ghost"
                    aria-label={`Settings of ${m.name}`}
                    disabled={!template}
                    onClick={() => setEditing(m)}
                  >
                    <Settings2 />
                  </Button>
                  <Button
                    size="mini"
                    variant="ghost"
                    aria-label={`Remove ${m.name}`}
                    onClick={() => {
                      setDeleteFiles(false);
                      setRemoving(m);
                    }}
                  >
                    <Trash2 />
                  </Button>
                </div>
              </li>
            );
          })}
        </ul>
      )}

      <h3 className="mb-2 font-semibold text-label">Add a module</h3>
      {templates.isError ? (
        <ErrorAlert>{errorMessage(templates.error)}</ErrorAlert>
      ) : (
        <ul className="grid gap-3 sm:grid-cols-2">
          {available.map((t) => (
            <li key={t.id} className="flex flex-col rounded-lg border border-border px-3.5 py-3">
              <p className="font-medium text-label">
                {t.name} <span className="font-normal text-meta text-text-muted">{t.version}</span>
              </p>
              <p className="mt-1 mb-3 flex-1 text-meta text-text-secondary">{t.description}</p>
              <Button
                size="mini"
                className="self-start"
                disabled={Boolean(unavailable)}
                onClick={() => setAdding(t)}
              >
                <Plus />
                Add {t.name}
              </Button>
            </li>
          ))}
        </ul>
      )}

      {adding && (
        <ModuleDialog
          server={server}
          template={adding}
          taken={modules.map((m) => m.name)}
          onClose={() => setAdding(null)}
          onSaved={refresh}
        />
      )}
      {editing && templateOf(editing) && (
        <ModuleDialog
          server={server}
          template={templateOf(editing) as ModuleTemplate}
          module={editing}
          taken={[]}
          onClose={() => setEditing(null)}
          onSaved={refresh}
        />
      )}
      {logs && <LogsDialog module={logs} onClose={() => setLogs(null)} />}
      {site && <SiteDialog server={server} module={site} onClose={() => setSite(null)} onSaved={refresh} />}
      <ConfirmDialog
        open={removing !== null}
        onOpenChange={(open) => !open && setRemoving(null)}
        title={`Remove ${removing?.name}?`}
        description="Its container is removed. Its folder stays unless you tick the box, so adding it again picks up where it left off."
        confirmLabel="Remove"
        pending={act.isPending}
        onConfirm={() => {
          const m = removing;
          setRemoving(null);
          if (m)
            act.mutate({
              work: () => client.removeModule({ id: m.id, deleteFiles }),
              done: `${m.name} removed.`,
            });
        }}
      >
        <label htmlFor="remove-module-files" className="flex items-center gap-2 text-label">
          <Checkbox
            id="remove-module-files"
            checked={deleteFiles}
            onCheckedChange={(v) => setDeleteFiles(v === true)}
          />
          Also delete its folder (<span className="font-mono">modules/{removing?.name}</span>)
        </label>
      </ConfirmDialog>
    </div>
  );
}

function initialValues(template: ModuleTemplate, module?: Module): Record<string, unknown> {
  const saved = module ? (JSON.parse(module.settingsJson || "{}") as Record<string, unknown>) : {};
  return Object.fromEntries(
    template.settings.map((s) => [
      s.key,
      s.key in saved ? saved[s.key] : JSON.parse(s.defaultJson || "null"),
    ]),
  );
}

/** Adds a module, or changes one's settings. */
function ModuleDialog({
  server,
  template,
  module,
  taken,
  onClose,
  onSaved,
}: {
  server: Server;
  template: ModuleTemplate;
  module?: Module;
  taken: string[];
  onClose: () => void;
  onSaved: () => void;
}) {
  const id = useId();
  const [name, setName] = useState(module?.name ?? (taken.includes(template.id) ? "" : template.id));
  const [lifecycle, setLifecycle] = useState(module?.lifecycle ?? template.lifecycle);
  const [memory, setMemory] = useState(String(module?.memoryMb ?? template.memoryMb));
  const [values, setValues] = useState(() => initialValues(template, module));
  const save = useMutation({
    mutationFn: async () => {
      const settingsJson = JSON.stringify(values);
      const memoryMb = Number(memory);
      if (module) {
        await client.updateModule({ id: module.id, settingsJson, lifecycle, memoryMb });
      } else {
        await client.addModule({
          serverId: server.id,
          template: template.id,
          name: name.trim(),
          settingsJson,
          lifecycle,
          memoryMb,
        });
      }
    },
    onSuccess: () => {
      toast.success(module ? `${module.name} saved.` : `${template.name} added.`);
      onSaved();
      onClose();
    },
  });
  const set = (key: string, value: unknown) => setValues((v) => ({ ...v, [key]: value }));
  const hint = LIFECYCLES.find((l) => l.value === lifecycle)?.hint;

  return (
    <Dialog open onOpenChange={(open) => !open && onClose()}>
      <DialogContent className="w-[560px]">
        <DialogHeader>
          <DialogTitle>{module ? `${module.name} settings` : `Add ${template.name}`}</DialogTitle>
          <DialogDescription>{template.description}</DialogDescription>
        </DialogHeader>
        <form
          onSubmit={(e) => {
            e.preventDefault();
            save.mutate();
          }}
        >
          {template.notes && !module && (
            <p className="mb-4 flex gap-2 rounded-md border border-info/25 bg-info-subtle px-3 py-2 text-meta text-text-secondary">
              <Info className="mt-0.5 size-3.5 shrink-0 text-info" aria-hidden />
              {template.notes}
            </p>
          )}
          {!module && (
            <Field
              label="Name"
              hint={`Its folder is modules/${name || "…"} in ${server.name}'s folder.`}
              value={name}
              onChange={(e) => setName(e.target.value)}
              pattern="[a-z0-9][a-z0-9-]{0,31}"
              required
            />
          )}
          <div className="mb-[17px]">
            <label htmlFor={`${id}-lifecycle`} className="mb-[7px] block text-label text-text-secondary">
              Runs
            </label>
            <Select id={`${id}-lifecycle`} value={lifecycle} onChange={(e) => setLifecycle(e.target.value)}>
              {LIFECYCLES.map((l) => (
                <option key={l.value} value={l.value}>
                  {l.label}
                </option>
              ))}
            </Select>
            {hint && <p className="mt-1.5 text-meta text-text-muted">{hint}</p>}
          </div>
          <Field
            label="Memory (MB)"
            type="number"
            min={128}
            max={16384}
            step={128}
            value={memory}
            onChange={(e) => setMemory(e.target.value)}
            required
          />
          {template.settings.map((s) => (
            <SettingField key={s.key} setting={s} value={values[s.key]} onChange={(v) => set(s.key, v)} />
          ))}
          {save.error && <ErrorAlert>{errorMessage(save.error)}</ErrorAlert>}
          <DialogFooter>
            <Button type="button" onClick={onClose}>
              Cancel
            </Button>
            <Button type="submit" variant="primary" disabled={save.isPending}>
              {save.isPending ? "Saving…" : module ? "Save" : `Add ${template.name}`}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}

function SettingField({
  setting,
  value,
  onChange,
}: {
  setting: ModuleSetting;
  value: unknown;
  onChange: (value: unknown) => void;
}) {
  const id = useId();
  switch (setting.kind) {
    case "boolean":
      return (
        <div className="mb-[17px]">
          <label htmlFor={id} className="flex items-start gap-2 text-label">
            <Checkbox
              id={id}
              className="mt-0.5"
              checked={value === true}
              onCheckedChange={(v) => onChange(v === true)}
            />
            <span>
              {setting.label}
              {setting.required && <span className="text-text-muted"> (required)</span>}
              {setting.help && <span className="mt-0.5 block text-meta text-text-muted">{setting.help}</span>}
            </span>
          </label>
        </div>
      );
    case "choice":
      return (
        <div className="mb-[17px]">
          <label htmlFor={id} className="mb-[7px] block text-label text-text-secondary">
            {setting.label}
          </label>
          <Select id={id} value={String(value ?? "")} onChange={(e) => onChange(e.target.value)}>
            {setting.choices.map((c) => (
              <option key={c} value={c}>
                {c}
              </option>
            ))}
          </Select>
          {setting.help && <p className="mt-1.5 text-meta text-text-muted">{setting.help}</p>}
        </div>
      );
    case "number":
      return (
        <Field
          label={setting.label}
          hint={setting.help || undefined}
          type="number"
          min={setting.min === undefined ? undefined : Number(setting.min)}
          max={setting.max === undefined ? undefined : Number(setting.max)}
          value={String(value ?? "")}
          onChange={(e) => onChange(e.target.value === "" ? null : Math.trunc(Number(e.target.value)))}
          required
        />
      );
    default:
      return (
        <Field
          label={setting.label}
          hint={setting.help || undefined}
          value={String(value ?? "")}
          onChange={(e) => onChange(e.target.value)}
          required={setting.required}
        />
      );
  }
}

/** What a module printed last. */
function LogsDialog({ module, onClose }: { module: Module; onClose: () => void }) {
  const logs = useQuery({
    queryKey: ["module-logs", module.id],
    queryFn: () => client.getModuleLogs({ id: module.id, lines: 300 }),
    refetchInterval: 5000,
  });
  return (
    <Dialog open onOpenChange={(open) => !open && onClose()}>
      <DialogContent className="w-[760px]">
        <DialogHeader>
          <DialogTitle>{module.name} log</DialogTitle>
          <DialogDescription>The last 300 lines it printed.</DialogDescription>
        </DialogHeader>
        {logs.isError ? (
          <ErrorAlert>{errorMessage(logs.error)}</ErrorAlert>
        ) : (
          <pre className="max-h-[60vh] overflow-auto whitespace-pre-wrap rounded-md bg-surface-subtle p-3 font-mono text-meta">
            {logs.data ? logs.data.lines.join("\n") || "Nothing yet." : "Loading…"}
          </pre>
        )}
      </DialogContent>
    </Dialog>
  );
}

/** A module's own address, and whether anyone can open it there. */
function SiteDialog({
  server,
  module,
  onClose,
  onSaved,
}: {
  server: Server;
  module: Module;
  onClose: () => void;
  onSaved: () => void;
}) {
  const id = useId();
  const routes = useQuery(routesQuery);
  const [host, setHost] = useState(module.webHost || "map");
  const [open, setOpen] = useState(module.webPublic);
  const error = siteError(host);
  const preview = siteAddresses(host, serverAddresses(server, routes.data ?? []));
  const save = useMutation({
    mutationFn: () => client.setModuleSite({ id: module.id, webHost: host.trim(), webPublic: open }),
    onSuccess: () => {
      toast.success(open ? `${module.name} is open at its own address.` : `${module.name}'s address is off.`);
      onSaved();
      onClose();
    },
  });
  return (
    <Dialog open onOpenChange={(o) => !o && onClose()}>
      <DialogContent className="w-[560px]">
        <DialogHeader>
          <DialogTitle>{module.name}'s own address</DialogTitle>
          <DialogDescription>
            A web address for its page that anyone can open, like a map to share with players.
          </DialogDescription>
        </DialogHeader>
        <form
          onSubmit={(e) => {
            e.preventDefault();
            if (!error) save.mutate();
          }}
        >
          <Field
            label="Address"
            hint="A prefix for the server's address (map), or a whole hostname."
            value={host}
            onChange={(e) => setHost(e.target.value)}
            error={host ? error : undefined}
            className="[&_input]:font-mono"
          />
          <p className="mb-4 text-meta text-text-secondary">
            {preview.length > 0 ? (
              <>
                It'll be at{" "}
                {preview.map((a, i) => (
                  <span key={a}>
                    {i > 0 && ", "}
                    <span className="font-mono">{a}</span>
                  </span>
                ))}
                .
              </>
            ) : (
              `${server.name} has no address yet for the prefix to go in front of. Give it one on Proxy & routes, or type a whole hostname.`
            )}
          </p>
          <div className="mb-4 flex items-start justify-between gap-5">
            <label htmlFor={id} className="text-label">
              <span className="block font-[550]">Open to anyone</span>
              <span className="mt-[3px] block text-meta text-text-muted">
                No sign-in: anyone with the address can see the page.
              </span>
            </label>
            <Switch id={id} checked={open} onCheckedChange={setOpen} />
          </div>
          <p className="mb-4 rounded-md border border-info/25 bg-info-subtle px-3 py-2 text-meta text-text-secondary">
            For the address to reach NetherNode, point its DNS where the panel's address points, and if a
            reverse proxy (like Pangolin) is in front of the panel, add the name there too, sent to
            NetherNode.
          </p>
          {save.error && <ErrorAlert>{errorMessage(save.error)}</ErrorAlert>}
          <DialogFooter>
            <Button type="button" onClick={onClose}>
              Cancel
            </Button>
            <Button type="submit" variant="primary" disabled={save.isPending || Boolean(error)}>
              {save.isPending ? "Saving…" : "Save"}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
