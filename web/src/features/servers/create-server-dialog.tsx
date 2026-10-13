import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useNavigate } from "@tanstack/react-router";
import { ExternalLink, Plus } from "lucide-react";
import { type FormEvent, useEffect, useId, useState } from "react";
import { toast } from "sonner";

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
import { CardTheme } from "@/gen/nethernode/v1/servers_pb";
import { errorMessage, serverClient } from "@/lib/api";

import { LOADERS, loaderName } from "./format";
import { THEME_NAMES } from "./world-banner";

const MEMORY_OPTIONS = [
  { mb: 2048, label: "2 GB — A small vanilla world" },
  { mb: 4096, label: "4 GB — Room to grow" },
  { mb: 6144, label: "6 GB — Plugins and friends" },
  { mb: 8192, label: "8 GB — Light modpacks" },
  { mb: 10240, label: "10 GB — Modded adventures" },
  { mb: 16384, label: "16 GB — Large modpacks" },
];

/** Suggestions only; any version string works. */
const VERSION_SUGGESTIONS = ["LATEST", "1.21.4", "1.21.1", "1.20.1", "1.19.2", "1.18.2", "1.16.5", "1.12.2"];

export const EULA_URL = "https://aka.ms/MinecraftEULA";

/** "A new world awaits." — creates a server and opens its workspace. */
export function CreateServerDialog({
  open,
  onOpenChange,
  preset,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  preset?: { name?: string; loader?: string; version?: string };
}) {
  const queryClient = useQueryClient();
  const navigate = useNavigate();
  const [name, setName] = useState("");
  const [loader, setLoader] = useState("paper");
  const [version, setVersion] = useState("LATEST");
  const [memoryMb, setMemoryMb] = useState(4096);
  const [theme, setTheme] = useState(CardTheme.NETHER);
  const [sleep, setSleep] = useState(true);
  const [startNow, setStartNow] = useState(true);
  const [eula, setEula] = useState(false);
  const [submitted, setSubmitted] = useState(false);
  const ids = { loader: useId(), memory: useId(), theme: useId(), versions: useId() };

  useEffect(() => {
    if (!open) return;
    setName(preset?.name ?? "");
    setLoader(preset?.loader ?? "paper");
    setVersion(preset?.version ?? "LATEST");
    setEula(false);
    setSubmitted(false);
  }, [open, preset]);

  const create = useMutation({
    mutationFn: () =>
      serverClient.createServer({
        name: name.trim(),
        loader,
        mcVersion: version.trim(),
        memoryMb,
        cardTheme: theme,
        eulaAccepted: eula,
        sleepWhenEmpty: sleep,
        startAfterCreate: startNow,
      }),
    onSuccess: async ({ server }) => {
      await queryClient.invalidateQueries({ queryKey: ["servers"] });
      onOpenChange(false);
      if (!server) return;
      toast.success(`${server.name} created. Installing…`);
      await navigate({ to: "/servers/$serverId", params: { serverId: server.id } });
    },
  });

  const nameError = submitted && !name.trim() ? "Give your server a name." : undefined;
  const versionError = submitted && !version.trim() ? "Enter a version, or LATEST." : undefined;
  const eulaError = submitted && !eula;

  const onSubmit = (e: FormEvent) => {
    e.preventDefault();
    setSubmitted(true);
    if (name.trim() && version.trim() && eula) create.mutate();
  };

  return (
    <Dialog
      open={open}
      onOpenChange={(next) => {
        if (!next) create.reset();
        onOpenChange(next);
      }}
    >
      <DialogContent>
        <DialogHeader>
          <DialogTitle>A new world awaits.</DialogTitle>
          <DialogDescription>Choose the basics. Your next server starts here.</DialogDescription>
        </DialogHeader>
        <form onSubmit={onSubmit} noValidate>
          {create.isError && <ErrorAlert>{errorMessage(create.error)}</ErrorAlert>}
          <Field
            label="Server name"
            placeholder="My awesome world"
            maxLength={40}
            value={name}
            onChange={(e) => setName(e.target.value)}
            error={nameError}
            autoFocus
          />
          <div className="grid grid-cols-2 gap-[13px] below-sm:grid-cols-1">
            <div className="mb-[17px]">
              <label htmlFor={ids.loader} className="mb-[7px] block text-label text-text-secondary">
                Server software
              </label>
              <Select id={ids.loader} value={loader} onChange={(e) => setLoader(e.target.value)}>
                {LOADERS.map((l) => (
                  <option key={l} value={l}>
                    {loaderName(l)}
                  </option>
                ))}
              </Select>
            </div>
            <Field
              label="Minecraft version"
              list={ids.versions}
              className="[&_input]:font-mono"
              value={version}
              onChange={(e) => setVersion(e.target.value)}
              error={versionError}
              hint="LATEST, or e.g. 1.20.1"
            />
            <datalist id={ids.versions}>
              {VERSION_SUGGESTIONS.map((v) => (
                <option key={v} value={v} />
              ))}
            </datalist>
          </div>
          <div className="mb-[17px]">
            <label htmlFor={ids.memory} className="mb-[7px] block text-label text-text-secondary">
              Memory allocation
            </label>
            <Select id={ids.memory} value={memoryMb} onChange={(e) => setMemoryMb(Number(e.target.value))}>
              {MEMORY_OPTIONS.map((m) => (
                <option key={m.mb} value={m.mb}>
                  {m.label}
                </option>
              ))}
            </Select>
            <p className="mt-1.5 text-meta text-text-muted">
              The Java heap; the container gets 25% more for overhead.
            </p>
          </div>
          <div className="mb-[17px]">
            <label htmlFor={ids.theme} className="mb-[7px] block text-label text-text-secondary">
              Card theme
            </label>
            <Select
              id={ids.theme}
              value={theme}
              onChange={(e) => setTheme(Number(e.target.value) as CardTheme)}
            >
              {Object.entries(THEME_NAMES).map(([value, label]) => (
                <option key={value} value={value}>
                  {label}
                </option>
              ))}
            </Select>
            <p className="mt-1.5 text-meta text-text-muted">
              Only changes the card artwork, never the world.
            </p>
          </div>

          <CheckRow checked={sleep} onChange={setSleep} title="Sleep when empty">
            Stops the server after 10 minutes without players, and starts it again when someone joins.
          </CheckRow>
          <CheckRow checked={startNow} onChange={setStartNow} title="Start when ready">
            Start the server as soon as it's installed.
          </CheckRow>
          <CheckRow checked={eula} onChange={setEula} title="I accept the Minecraft EULA" invalid={eulaError}>
            Required by Mojang to run a server.{" "}
            <a
              href={EULA_URL}
              target="_blank"
              rel="noreferrer noopener"
              className="inline-flex items-center gap-0.5 text-text-primary underline underline-offset-2"
            >
              Read the EULA
              <ExternalLink className="size-3" aria-hidden />
            </a>
          </CheckRow>
          {eulaError && <p className="-mt-2 mb-[17px] text-meta text-danger">Accept the EULA to continue.</p>}

          <DialogFooter>
            <Button onClick={() => onOpenChange(false)}>Cancel</Button>
            <Button type="submit" variant="primary" disabled={create.isPending}>
              <Plus />
              {create.isPending ? "Creating…" : "Create server"}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}

function CheckRow({
  checked,
  onChange,
  title,
  children,
  invalid = false,
}: {
  checked: boolean;
  onChange: (checked: boolean) => void;
  title: string;
  children: React.ReactNode;
  invalid?: boolean;
}) {
  const id = useId();
  return (
    <div className="mb-[17px] flex items-start gap-3">
      <Checkbox
        id={id}
        checked={checked}
        onCheckedChange={(v) => onChange(v === true)}
        aria-invalid={invalid || undefined}
        className="mt-0.5 aria-invalid:border-danger"
      />
      <div className="text-label">
        <label htmlFor={id} className="block font-[550]">
          {title}
        </label>
        <p className="mt-0.5 text-meta text-text-muted">{children}</p>
      </div>
    </div>
  );
}
