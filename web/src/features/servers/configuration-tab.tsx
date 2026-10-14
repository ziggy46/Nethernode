import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useNavigate } from "@tanstack/react-router";
import { Save, Settings, Trash2, TriangleAlert } from "lucide-react";
import { type FormEvent, lazy, Suspense, useEffect, useId, useState } from "react";
import { toast } from "sonner";

import { Panel, PanelHeader } from "@/components/page";
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
import { ErrorAlert, Field, Input, Select } from "@/components/ui/field";
import { Switch } from "@/components/ui/switch";
import { CardTheme, type Server } from "@/gen/nethernode/v1/servers_pb";
import { errorMessage, serverClient } from "@/lib/api";
import { useCanManage } from "@/lib/session";

import { LOADERS, loaderName } from "./format";
import { THEME_NAMES } from "./world-banner";

const GameSettings = lazy(() => import("./game-settings"));

interface FormState {
  name: string;
  motd: string;
  loader: string;
  mcVersion: string;
  loaderVersion: string;
  memoryGb: string;
  maxPlayers: string;
  sleepWhenEmpty: boolean;
  idleMinutes: string;
  autoStart: boolean;
  restartOnCrash: boolean;
  useAikarFlags: boolean;
  cardTheme: CardTheme;
  javaVersion: string;
  image: string;
}

function fromServer(server: Server): FormState {
  const s = server.settings;
  return {
    name: s?.name ?? server.name,
    motd: s?.motd ?? "",
    loader: s?.loader ?? server.loader,
    mcVersion: s?.mcVersion ?? server.mcVersion,
    loaderVersion: s?.loaderVersion ?? "",
    memoryGb: String((s?.memoryMb ?? 2048) / 1024),
    maxPlayers: String(s?.maxPlayers ?? 20),
    sleepWhenEmpty: s?.sleepWhenEmpty ?? false,
    idleMinutes: String(s?.idleMinutes ?? 10),
    autoStart: s?.autoStart ?? false,
    restartOnCrash: s?.restartOnCrash ?? true,
    useAikarFlags: s?.useAikarFlags ?? true,
    cardTheme: s?.cardTheme ?? CardTheme.NETHER,
    javaVersion: s?.javaVersion ? String(s.javaVersion) : "",
    image: s?.image?.startsWith("itzg/minecraft-server") ? "" : (s?.image ?? ""),
  };
}

export function ConfigurationTab({ server }: { server: Server }) {
  const queryClient = useQueryClient();
  const canManage = useCanManage();
  const [form, setForm] = useState(() => fromServer(server));
  const isModpack = server.modpack !== undefined;
  const set = <K extends keyof FormState>(key: K, value: FormState[K]) =>
    setForm((f) => ({ ...f, [key]: value }));
  const ids = {
    loader: useId(),
    theme: useId(),
    java: useId(),
    sleep: useId(),
    auto: useId(),
    crash: useId(),
    aikar: useId(),
  };

  useEffect(() => setForm(fromServer(server)), [server]);

  const memoryMb = Math.round(Number(form.memoryGb) * 1024);
  const errors = {
    name: form.name.trim() ? undefined : "Required.",
    memory: Number.isFinite(memoryMb) && memoryMb >= 512 && memoryMb <= 65536 ? undefined : "0.5–64 GB.",
    players: /^\d+$/.test(form.maxPlayers) && Number(form.maxPlayers) >= 1 ? undefined : "At least 1.",
    idle: /^\d+$/.test(form.idleMinutes) && Number(form.idleMinutes) >= 1 ? undefined : "At least 1 minute.",
    version: form.mcVersion.trim() ? undefined : "Required.",
  };
  const valid = Object.values(errors).every((e) => !e);

  const save = useMutation({
    mutationFn: () =>
      serverClient.updateServer({
        id: server.id,
        settings: {
          name: form.name.trim(),
          motd: form.motd.trim(),
          loader: form.loader,
          mcVersion: form.mcVersion.trim(),
          loaderVersion: form.loaderVersion.trim(),
          image: form.image.trim(),
          javaVersion: form.javaVersion ? Number(form.javaVersion) : undefined,
          memoryMb,
          maxPlayers: Number(form.maxPlayers),
          sleepWhenEmpty: form.sleepWhenEmpty,
          idleMinutes: Number(form.idleMinutes),
          autoStart: form.autoStart,
          restartOnCrash: form.restartOnCrash,
          useAikarFlags: form.useAikarFlags,
          cardTheme: form.cardTheme,
        },
      }),
    onSuccess: async ({ server: saved }) => {
      await queryClient.invalidateQueries({ queryKey: ["servers"] });
      toast.success(
        saved?.restartRequired
          ? "Configuration saved. Restart the server to apply it."
          : "Configuration saved.",
      );
    },
  });

  const onSubmit = (e: FormEvent) => {
    e.preventDefault();
    if (valid) save.mutate();
  };

  return (
    <div className="flex max-w-[720px] flex-col gap-[18px]">
      <form onSubmit={onSubmit} noValidate>
        <Panel>
          <PanelHeader title="Server configuration" icon={<Settings />} />
          {save.isError && <ErrorAlert>{errorMessage(save.error)}</ErrorAlert>}
          <fieldset disabled={!canManage} className="disabled:opacity-60">
            <Field
              label="Server name"
              maxLength={40}
              value={form.name}
              onChange={(e) => set("name", e.target.value)}
              error={errors.name}
            />
            <Field
              label="Message of the day"
              maxLength={120}
              hint="Shown under the server's name in the server list."
              value={form.motd}
              onChange={(e) => set("motd", e.target.value)}
            />
            <div className="grid grid-cols-2 gap-[13px] below-sm:grid-cols-1">
              <Field
                label="Memory allocation (GB)"
                inputMode="decimal"
                value={form.memoryGb}
                onChange={(e) => set("memoryGb", e.target.value)}
                error={errors.memory}
              />
              <Field
                label="Maximum players"
                inputMode="numeric"
                value={form.maxPlayers}
                onChange={(e) => set("maxPlayers", e.target.value)}
                error={errors.players}
              />
            </div>

            <ToggleRow
              id={ids.sleep}
              checked={form.sleepWhenEmpty}
              onChange={(v) => set("sleepWhenEmpty", v)}
              title="Sleep when empty"
            >
              Stop after a stretch with no players; start again when someone joins.
            </ToggleRow>
            {form.sleepWhenEmpty && (
              <div className="mb-[17px] flex items-center gap-2 pl-0.5 text-label text-text-secondary">
                <label htmlFor={`${ids.sleep}-minutes`}>Sleep after</label>
                <Input
                  id={`${ids.sleep}-minutes`}
                  inputMode="numeric"
                  className="h-9 w-20"
                  value={form.idleMinutes}
                  aria-invalid={errors.idle ? true : undefined}
                  onChange={(e) => set("idleMinutes", e.target.value)}
                />
                <span>minutes without players</span>
              </div>
            )}
            <ToggleRow
              id={ids.auto}
              checked={form.autoStart}
              onChange={(v) => set("autoStart", v)}
              title="Start with NetherNode"
            >
              Bring this server online when NetherNode starts.
            </ToggleRow>
            <ToggleRow
              id={ids.crash}
              checked={form.restartOnCrash}
              onChange={(v) => set("restartOnCrash", v)}
              title="Restart after a crash"
            >
              Up to 3 times in 10 minutes.
            </ToggleRow>

            <div className="mb-[17px]">
              <label htmlFor={ids.theme} className="mb-[7px] block text-label text-text-secondary">
                Card theme
              </label>
              <Select
                id={ids.theme}
                value={form.cardTheme}
                onChange={(e) => set("cardTheme", Number(e.target.value) as CardTheme)}
              >
                {Object.entries(THEME_NAMES).map(([value, label]) => (
                  <option key={value} value={value}>
                    {label}
                  </option>
                ))}
              </Select>
            </div>

            <details className="mb-[17px] rounded-md border border-divider px-3 py-2.5">
              <summary className="cursor-pointer text-label text-text-secondary">
                Advanced: software, Java, and image
              </summary>
              <div className="mt-4">
                <p className="mb-4 flex gap-2 text-meta text-warning">
                  <TriangleAlert className="mt-px size-3.5 shrink-0" aria-hidden />
                  Changing the software or version on an existing world can break it. Back up first.
                </p>
                <div className="grid grid-cols-2 gap-[13px] below-sm:grid-cols-1">
                  <div className="mb-[17px]">
                    <label htmlFor={ids.loader} className="mb-[7px] block text-label text-text-secondary">
                      Server software
                    </label>
                    <Select
                      id={ids.loader}
                      value={form.loader}
                      disabled={isModpack}
                      onChange={(e) => set("loader", e.target.value)}
                    >
                      {(isModpack ? [form.loader] : LOADERS).map((l) => (
                        <option key={l} value={l}>
                          {loaderName(l)}
                        </option>
                      ))}
                    </Select>
                  </div>
                  <Field
                    label="Minecraft version"
                    className="[&_input]:font-mono"
                    value={form.mcVersion}
                    disabled={isModpack}
                    hint={isModpack ? "Set by the modpack." : undefined}
                    onChange={(e) => set("mcVersion", e.target.value)}
                    error={errors.version}
                  />
                </div>
                <Field
                  label="Loader or build version"
                  className="[&_input]:font-mono"
                  hint="Leave empty for the recommended or latest build."
                  value={form.loaderVersion}
                  onChange={(e) => set("loaderVersion", e.target.value)}
                />
                <div className="mb-[17px]">
                  <label htmlFor={ids.java} className="mb-[7px] block text-label text-text-secondary">
                    Java version
                  </label>
                  <Select
                    id={ids.java}
                    value={form.javaVersion}
                    onChange={(e) => set("javaVersion", e.target.value)}
                  >
                    <option value="">Automatic (matches the Minecraft version)</option>
                    {[8, 11, 17, 21, 25].map((j) => (
                      <option key={j} value={j}>
                        Java {j}
                      </option>
                    ))}
                  </Select>
                </div>
                <Field
                  label="Custom image"
                  className="[&_input]:font-mono"
                  placeholder="itzg/minecraft-server (default)"
                  hint="Leave empty to use itzg/minecraft-server with the right Java version."
                  value={form.image}
                  onChange={(e) => set("image", e.target.value)}
                />
                <ToggleRow
                  id={ids.aikar}
                  checked={form.useAikarFlags}
                  onChange={(v) => set("useAikarFlags", v)}
                  title="Aikar's JVM flags"
                >
                  Garbage-collection tuning that reduces lag spikes.
                </ToggleRow>
              </div>
            </details>

            {canManage && (
              <div className="flex items-center gap-3">
                <Button type="submit" variant="primary" disabled={save.isPending || !valid}>
                  <Save />
                  {save.isPending ? "Saving…" : "Save configuration"}
                </Button>
                <p className="text-meta text-text-muted">
                  Most changes apply the next time the server starts.
                </p>
              </div>
            )}
          </fieldset>
        </Panel>
      </form>
      {canManage && (
        <Suspense fallback={<div className="h-48 animate-pulse rounded-lg bg-surface-subtle" aria-busy />}>
          <GameSettings server={server} />
        </Suspense>
      )}
      {canManage && <DangerZone server={server} />}
    </div>
  );
}

function ToggleRow({
  id,
  checked,
  onChange,
  title,
  children,
}: {
  id: string;
  checked: boolean;
  onChange: (value: boolean) => void;
  title: string;
  children: React.ReactNode;
}) {
  return (
    <div className="mb-[17px] flex items-start justify-between gap-5">
      <label htmlFor={id} className="text-label">
        <span className="block font-[550]">{title}</span>
        <span className="mt-0.5 block text-meta text-text-muted">{children}</span>
      </label>
      <Switch id={id} checked={checked} onCheckedChange={onChange} />
    </div>
  );
}

function DangerZone({ server }: { server: Server }) {
  const queryClient = useQueryClient();
  const navigate = useNavigate();
  const [open, setOpen] = useState(false);
  const [confirmName, setConfirmName] = useState("");
  const [deleteData, setDeleteData] = useState(false);
  const dataId = useId();

  const remove = useMutation({
    mutationFn: () => serverClient.deleteServer({ id: server.id, deleteData }),
    onSuccess: async () => {
      setOpen(false);
      await queryClient.invalidateQueries({ queryKey: ["servers"] });
      toast.success(`${server.name} deleted.`);
      await navigate({ to: "/servers" });
    },
  });

  return (
    <Panel className="border-danger/30">
      <PanelHeader title="Danger zone" icon={<TriangleAlert />} />
      <div className="flex items-center justify-between gap-5 below-sm:flex-col below-sm:items-start">
        <div className="text-label">
          <p className="font-[550]">Delete this server</p>
          <p className="mt-0.5 text-meta text-text-muted">
            Stops it immediately and removes it from NetherNode.
          </p>
        </div>
        <Button variant="danger" onClick={() => setOpen(true)}>
          <Trash2 />
          Delete server
        </Button>
      </div>
      <Dialog
        open={open}
        onOpenChange={(next) => {
          setOpen(next);
          if (!next) {
            setConfirmName("");
            setDeleteData(false);
            remove.reset();
          }
        }}
      >
        <DialogContent>
          <DialogHeader>
            <DialogTitle>Delete {server.name}?</DialogTitle>
            <DialogDescription>
              Players are disconnected and the server's routes are removed.
            </DialogDescription>
          </DialogHeader>
          {remove.isError && <ErrorAlert>{errorMessage(remove.error)}</ErrorAlert>}
          <div className="mb-[17px] flex items-start gap-3">
            <Checkbox
              id={dataId}
              checked={deleteData}
              onCheckedChange={(v) => setDeleteData(v === true)}
              className="mt-0.5"
            />
            <label htmlFor={dataId} className="text-label">
              <span className="block font-[550]">Also delete the world and files</span>
              <span className="mt-0.5 block text-meta text-text-muted">
                {deleteData
                  ? "Permanent. The world can't be recovered."
                  : "Files stay on disk in the data directory."}
              </span>
            </label>
          </div>
          <Field
            label={`Type ${server.name} to confirm`}
            autoComplete="off"
            value={confirmName}
            onChange={(e) => setConfirmName(e.target.value)}
          />
          <DialogFooter>
            <Button onClick={() => setOpen(false)}>Cancel</Button>
            <Button
              variant="danger"
              disabled={confirmName !== server.name || remove.isPending}
              onClick={() => remove.mutate()}
            >
              <Trash2 />
              {remove.isPending ? "Deleting…" : "Delete server"}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </Panel>
  );
}
