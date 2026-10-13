/**
 * The server.properties form: Minecraft's own settings, grouped and
 * validated, with a raw-file view. Loaded on demand from the Configuration
 * tab.
 */
import { Code, ConnectError } from "@connectrpc/connect";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { FileCode, Gamepad2, ListChecks, Lock, RotateCcw, Save } from "lucide-react";
import { lazy, Suspense, useId, useRef, useState } from "react";
import { toast } from "sonner";

import { ConfirmDialog } from "@/components/confirm-dialog";
import { EmptyState, Panel } from "@/components/page";
import { Button } from "@/components/ui/button";
import { ErrorAlert, Input, Select } from "@/components/ui/field";
import { Switch } from "@/components/ui/switch";
import { PropertyType, type Server, type ServerProperty, ServerStatus } from "@/gen/nethernode/v1/servers_pb";
import { errorMessage, serverClient } from "@/lib/api";
import { cn } from "@/lib/utils";

import { fileClient } from "../files/api";
import type { CodeEditorHandle } from "../files/code-editor";
import { filesKey } from "../files/queries";
import { useServerAction } from "./use-server-actions";

const CodeEditor = lazy(() => import("../files/code-editor"));

const COLLAPSED = new Set(["Advanced", "Other"]);
const FILE = "server.properties";

const propertiesKey = (serverId: string) => [...filesKey(serverId), "properties"] as const;

/** A problem with `value` for `p`, if any. */
function invalid(p: ServerProperty, value: string): string | null {
  if (p.type === PropertyType.INT) {
    if (!/^-?\d+$/.test(value.trim())) return "Enter a whole number.";
    const n = Number(value);
    if (p.min !== undefined && n < Number(p.min)) return `At least ${p.min}.`;
    if (p.max !== undefined && n > Number(p.max)) return `At most ${p.max}.`;
  }
  if (/[\r\n]/.test(value)) return "Use a single line.";
  return null;
}

export default function GameSettings({ server }: { server: Server }) {
  const client = useQueryClient();
  const query = useQuery({
    queryKey: propertiesKey(server.id),
    queryFn: () => serverClient.getServerProperties({ id: server.id }),
  });
  const [edits, setEdits] = useState<Record<string, string>>({});
  const [view, setView] = useState<"form" | "raw">("form");
  const [stale, setStale] = useState(false);
  const [savedWhileRunning, setSavedWhileRunning] = useState(false);
  const [pendingView, setPendingView] = useState<"form" | "raw" | null>(null);
  const action = useServerAction(server.id, server.name);
  const running = server.status === ServerStatus.RUNNING;

  const properties = query.data?.properties ?? [];
  const changes = Object.entries(edits).filter(
    ([key, value]) => properties.find((p) => p.key === key)?.value !== value,
  );
  const errors = Object.fromEntries(
    changes.flatMap(([key, value]) => {
      const p = properties.find((x) => x.key === key);
      const problem = p ? invalid(p, value) : null;
      return problem ? [[key, problem]] : [];
    }),
  );

  const save = useMutation({
    mutationFn: () =>
      serverClient.updateServerProperties({
        id: server.id,
        version: query.data?.version ?? "",
        values: Object.fromEntries(changes),
      }),
    onSuccess: (response) => {
      if (response.properties) client.setQueryData(propertiesKey(server.id), response.properties);
      setEdits({});
      setSavedWhileRunning(running);
      void client.invalidateQueries({ queryKey: [...filesKey(server.id), "list"] });
      toast.success(
        running ? "Game settings saved. Restart the server to apply them." : "Game settings saved.",
      );
    },
    onError: (err) => {
      if (ConnectError.from(err).code === Code.Aborted) setStale(true);
    },
  });

  const reload = async () => {
    setStale(false);
    setEdits({});
    save.reset();
    await query.refetch();
  };

  const groups = [...new Set(properties.map((p) => p.group))];

  return (
    <Panel>
      <div className="mb-[17px] flex flex-wrap items-center justify-between gap-3">
        <h2 className="flex items-center gap-2 font-semibold text-label">
          <Gamepad2 className="size-[15px] text-text-muted" aria-hidden />
          Game settings
        </h2>
        {query.data?.exists && (
          <fieldset className="flex rounded-md border border-border p-0.5">
            <legend className="sr-only">View</legend>
            {(
              [
                ["form", "Form", <ListChecks key="f" />],
                ["raw", "Raw file", <FileCode key="r" />],
              ] as const
            ).map(([value, label, icon]) => (
              <Button
                key={value}
                size="mini"
                variant={view === value ? "accent" : "ghost"}
                aria-pressed={view === value}
                onClick={() => {
                  if (value === view) return;
                  if (changes.length > 0) setPendingView(value);
                  else setView(value);
                }}
              >
                {icon}
                {label}
              </Button>
            ))}
          </fieldset>
        )}
      </div>
      <p className="mb-[17px] text-meta text-text-muted">
        Minecraft's own settings from {FILE}. The server reads them when it starts, so changes apply after a
        restart.
      </p>

      {savedWhileRunning && running && changes.length === 0 && (
        <div className="mb-[17px] flex flex-wrap items-center gap-3 rounded-md border border-warning/30 bg-warning-subtle px-3 py-2.5 text-label text-warning">
          <span className="flex-1">Restart {server.name} to apply the new settings.</span>
          <Button
            size="mini"
            disabled={action.isPending}
            onClick={() => {
              action.mutate("restart");
              setSavedWhileRunning(false);
            }}
          >
            <RotateCcw />
            Restart now
          </Button>
        </div>
      )}

      {query.isPending ? (
        <div className="h-48 animate-pulse rounded-md bg-surface" aria-busy />
      ) : query.isError ? (
        <ErrorAlert>{errorMessage(query.error)}</ErrorAlert>
      ) : !query.data.exists ? (
        <EmptyState compact icon={<Gamepad2 />} title="No game settings yet">
          Minecraft creates {FILE} the first time the server starts. Start it once, then come back here.
        </EmptyState>
      ) : view === "raw" ? (
        <RawEditor serverId={server.id} onSaved={() => setSavedWhileRunning(running)} />
      ) : (
        <form
          noValidate
          onSubmit={(e) => {
            e.preventDefault();
            if (changes.length > 0 && Object.keys(errors).length === 0) save.mutate();
          }}
        >
          {stale ? (
            <ErrorAlert>
              <span className="flex-1">
                {FILE} changed since this form loaded (the server rewrites it when starting). Reload to see
                the current values; your edits will be cleared.
              </span>
              <Button size="mini" onClick={reload}>
                Reload
              </Button>
            </ErrorAlert>
          ) : (
            save.isError && <ErrorAlert>{errorMessage(save.error)}</ErrorAlert>
          )}
          {groups.map((group) => {
            const fields = properties.filter((p) => p.group === group);
            const body = (
              <div key="fields" className="divide-y divide-divider">
                {fields.map((p) => (
                  <PropertyField
                    key={p.key}
                    property={p}
                    value={edits[p.key] ?? p.value}
                    changed={edits[p.key] !== undefined && edits[p.key] !== p.value}
                    error={errors[p.key]}
                    onChange={(value) => setEdits((e) => ({ ...e, [p.key]: value }))}
                  />
                ))}
              </div>
            );
            return COLLAPSED.has(group) ? (
              <details key={group} className="mb-[17px] rounded-md border border-divider px-3 py-2.5">
                <summary className="cursor-pointer text-label text-text-secondary">
                  {group} <span className="text-text-muted">({fields.length})</span>
                </summary>
                <div className="mt-2">{body}</div>
              </details>
            ) : (
              <section key={group} className="mb-[17px]" aria-label={group}>
                <h3 className="mb-1 font-[650] text-eyebrow text-text-muted uppercase">{group}</h3>
                {body}
              </section>
            );
          })}
          <div className="sticky bottom-0 -mx-[19px] -mb-[19px] flex items-center gap-3 border-divider border-t bg-surface-subtle px-[19px] py-3">
            <Button
              type="submit"
              variant="primary"
              disabled={save.isPending || changes.length === 0 || Object.keys(errors).length > 0 || stale}
            >
              <Save />
              {save.isPending ? "Saving…" : "Save game settings"}
            </Button>
            {changes.length > 0 && (
              <>
                <span className="text-meta text-text-muted" role="status">
                  {changes.length} unsaved change{changes.length === 1 ? "" : "s"}
                </span>
                <Button variant="ghost" size="mini" onClick={() => setEdits({})}>
                  Reset
                </Button>
              </>
            )}
          </div>
        </form>
      )}
      <ConfirmDialog
        open={pendingView !== null}
        onOpenChange={(open) => {
          if (!open) setPendingView(null);
        }}
        title="Discard unsaved game settings?"
        description={`You have ${changes.length} unsaved change${changes.length === 1 ? "" : "s"} in the form.`}
        confirmLabel="Discard changes"
        onConfirm={() => {
          setEdits({});
          if (pendingView) setView(pendingView);
          setPendingView(null);
        }}
      />
    </Panel>
  );
}

function PropertyField({
  property: p,
  value,
  changed,
  error,
  onChange,
}: {
  property: ServerProperty;
  value: string;
  changed: boolean;
  error: string | undefined;
  onChange: (value: string) => void;
}) {
  const id = useId();
  const helpId = `${id}-help`;
  const managed = p.managedReason !== "";
  const label = (
    <label htmlFor={id} className="block text-label text-text-primary">
      {p.label}
      {changed && <span className="ml-1.5 text-meta text-primary">• changed</span>}
    </label>
  );
  const help = (
    <p id={helpId} className={cn("mt-0.5 text-meta", error ? "text-danger" : "text-text-muted")}>
      {error ?? (managed ? p.managedReason : p.help)}
      {!error && <span className="ml-1.5 font-mono opacity-80">{p.key}</span>}
    </p>
  );

  let control: React.ReactNode;
  if (managed || p.secret) {
    control = (
      <span id={id} className="flex items-center gap-1.5 font-mono text-label text-text-secondary">
        <Lock className="size-3 text-text-muted" aria-label="Managed by NetherNode" />
        {p.secret ? "hidden" : value || "—"}
      </span>
    );
  } else if (p.type === PropertyType.BOOL) {
    control = (
      <Switch
        id={id}
        aria-describedby={helpId}
        checked={value === "true"}
        onCheckedChange={(v) => onChange(v ? "true" : "false")}
      />
    );
  } else if (p.type === PropertyType.CHOICE) {
    control = (
      <Select
        id={id}
        aria-describedby={helpId}
        className="h-9 w-40"
        value={value}
        onChange={(e) => onChange(e.target.value)}
      >
        {!p.options.includes(value) && <option value={value}>{value}</option>}
        {p.options.map((o) => (
          <option key={o} value={o}>
            {o}
          </option>
        ))}
      </Select>
    );
  } else {
    control = (
      <Input
        id={id}
        aria-describedby={helpId}
        aria-invalid={error ? true : undefined}
        inputMode={p.type === PropertyType.INT ? "numeric" : undefined}
        className={cn(
          "h-9 font-mono text-label",
          p.type === PropertyType.INT ? "w-28" : "w-64 below-sm:w-full",
        )}
        value={value}
        spellCheck={false}
        onChange={(e) => onChange(e.target.value)}
      />
    );
  }

  return (
    <div className="flex items-center justify-between gap-5 py-3 below-sm:flex-col below-sm:items-start below-sm:gap-2">
      <div className="min-w-0">
        {label}
        {help}
      </div>
      <div className="shrink-0">{control}</div>
    </div>
  );
}

function RawEditor({ serverId, onSaved }: { serverId: string; onSaved: () => void }) {
  const client = useQueryClient();
  const file = useQuery({
    queryKey: [...filesKey(serverId), "read", FILE],
    queryFn: () => fileClient.readFile({ serverId, path: FILE }),
    staleTime: Number.POSITIVE_INFINITY,
    gcTime: 0,
  });
  const editor = useRef<CodeEditorHandle>(null);
  const [dirty, setDirty] = useState(false);
  const save = useMutation({
    mutationFn: () =>
      fileClient.writeFile({
        serverId,
        path: FILE,
        content: new TextEncoder().encode(editor.current?.value() ?? ""),
        expectedVersion: file.data?.version,
      }),
    onSuccess: async () => {
      editor.current?.markSaved();
      await client.invalidateQueries({ queryKey: filesKey(serverId) });
      onSaved();
      toast.success(`Saved ${FILE}.`);
    },
  });
  if (file.isPending) return <div className="h-64 animate-pulse rounded-md bg-console" aria-busy />;
  if (file.isError) return <ErrorAlert>{errorMessage(file.error)}</ErrorAlert>;
  return (
    <div>
      {save.isError && (
        <ErrorAlert>
          {ConnectError.from(save.error).code === Code.Aborted
            ? `${FILE} changed since it was opened. Switch to the form and back to reload it.`
            : errorMessage(save.error)}
        </ErrorAlert>
      )}
      <p className="mb-2 text-meta text-text-muted">
        NetherNode resets the port, RCON, message of the day, and player limit on every start. Change the
        message and player limit under Server configuration above.
      </p>
      <div className="h-[420px] overflow-hidden rounded-md border border-border">
        <Suspense fallback={<div className="h-full animate-pulse bg-console" aria-busy />}>
          <CodeEditor
            ref={editor}
            initial={new TextDecoder().decode(file.data.content)}
            language="properties"
            label={`Contents of ${FILE}`}
            onDirtyChange={setDirty}
            onSave={() => {
              if (dirty) save.mutate();
            }}
          />
        </Suspense>
      </div>
      <div className="mt-3 flex items-center gap-3">
        <Button variant="primary" disabled={!dirty || save.isPending} onClick={() => save.mutate()}>
          <Save />
          {save.isPending ? "Saving…" : `Save ${FILE}`}
        </Button>
        {dirty && <span className="text-meta text-text-muted">Unsaved changes</span>}
      </div>
    </div>
  );
}
