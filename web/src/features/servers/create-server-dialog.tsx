import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useNavigate } from "@tanstack/react-router";
import { ArrowLeft, ArrowRight, Plus } from "lucide-react";
import { useCallback, useEffect, useState } from "react";
import { toast } from "sonner";

import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { ErrorAlert } from "@/components/ui/field";
import { nodesQuery } from "@/features/nodes/queries";
import { proxyClient } from "@/features/proxy/client";
import { Platform } from "@/gen/nethernode/v1/catalog_pb";
import { NodeStatus } from "@/gen/nethernode/v1/nodes_pb";
import type { ServerTemplate } from "@/gen/nethernode/v1/servers_pb";
import { errorMessage, serverClient } from "@/lib/api";
import { healthQuery } from "@/lib/health";
import { useCanManage } from "@/lib/session";
import { cn } from "@/lib/utils";

import { PLATFORM_NAMES } from "../catalog/queries";
import { domainsQuery } from "../domains/queries";
import { routesQuery } from "../proxy/queries";
import { type Draft, domainAddress, MORE_SOFTWARE, newDraft, SOFTWARE, subdomainError } from "./create/draft";
import {
  ModpackStep,
  NetworkStep,
  ResourcesStep,
  ReviewStep,
  SoftwareStep,
  VersionStep,
} from "./create/steps";
import { loaderName } from "./format";
import { FromTemplateDialog, TemplatePicker } from "./templates";

export { EULA_URL } from "./create/steps";

const STEPS = ["Type", "Version", "Resources", "Network", "Review"] as const;

/** "A new world awaits." — a five-step guide to a new server. */
export function CreateServerDialog({
  open,
  onOpenChange,
  preset,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** Start from a type (and, for a modpack, which one). */
  preset?: Partial<Draft>;
}) {
  const queryClient = useQueryClient();
  const navigate = useNavigate();
  const health = useQuery(healthQuery);
  const host = useQuery({
    queryKey: ["host-info"],
    queryFn: () => serverClient.getHostInfo({}),
    staleTime: 60_000,
    enabled: open,
  });
  const [draft, setDraft] = useState(() => newDraft(preset));
  const [step, setStep] = useState(0);
  const [template, setTemplate] = useState<ServerTemplate | null>(null);
  const update = useCallback((patch: Partial<Draft>) => setDraft((d) => ({ ...d, ...patch })), []);
  // Machines that have joined (one waiting to join can't run anything yet).
  const machines = (useQuery({ ...nodesQuery, enabled: open }).data?.nodes ?? []).filter(
    (n) => n.status !== NodeStatus.WAITING,
  );
  const machine = machines.find((n) => n.id === draft.nodeId);
  // Admins can also manage the proxy (routes and listeners).
  const canManageProxy = useCanManage();

  useEffect(() => {
    if (!open) return;
    setDraft(newDraft(preset));
    // A modpack picked elsewhere skips straight to its version.
    setStep(preset?.pack ? 1 : 0);
  }, [open, preset]);

  const isPack = draft.software === "modpack";
  const softwareName =
    [...SOFTWARE, ...MORE_SOFTWARE].find((s) => s.id === draft.software)?.name ?? draft.software;
  const port = Number(draft.port);
  const domains = useQuery({ ...domainsQuery, enabled: open && canManageProxy }).data ?? [];
  const routes = useQuery({ ...routesQuery, enabled: open && canManageProxy }).data ?? [];
  const address = domainAddress(draft, domains);
  const taken = address ? routes.find((r) => r.hostname === address) : undefined;
  const takenBy = taken ? taken.serverName || "another server" : undefined;
  const problems: (string | null)[] = [
    null,
    isPack
      ? !draft.pack?.project
        ? "Choose a modpack."
        : draft.pack.platform === Platform.CURSEFORGE && !draft.pack.versionId && !draft.pack.mcVersion
          ? "Choose the pack's Minecraft version."
          : null
      : !draft.mcVersion.trim()
        ? "Choose a Minecraft version."
        : null,
    null,
    draft.network === "domain" && !address
      ? "Add a domain first, or choose another option."
      : draft.network === "domain" && subdomainError(draft.subdomain)
        ? "Fix the name before the domain."
        : draft.network === "domain" && draft.subdomain.trim() && takenBy
          ? `${address} already leads to ${takenBy}. Choose another name.`
          : draft.network === "hostname" && !/^[a-z0-9.-]+\.[a-z]{2,}$/i.test(draft.hostname.trim())
            ? "Enter a hostname like play.example.com."
            : draft.network === "port" && !(port >= 1024 && port <= 65535)
              ? "Choose a port between 1024 and 65535."
              : null,
    !draft.name.trim()
      ? "Give your server a name."
      : draft.network === "domain" && takenBy
        ? `Players would join with ${address}, which already leads to ${takenBy}. Pick another name, or change it on the Network step.`
        : !draft.eula
          ? "Accept the EULA to continue."
          : null,
  ];
  const problem = problems[step];

  const create = useMutation({
    mutationFn: async () => {
      const pack = draft.pack;
      const { server } = await serverClient.createServer({
        name: draft.name.trim(),
        loader: isPack
          ? pack?.platform === Platform.CURSEFORGE
            ? "curseforge"
            : "modrinth"
          : draft.software,
        mcVersion: isPack ? (pack?.mcVersion ?? "") : draft.mcVersion.trim(),
        loaderVersion: isPack ? "" : draft.loaderVersion,
        memoryMb: draft.memoryMb,
        cardTheme: draft.theme,
        eulaAccepted: draft.eula,
        sleepWhenEmpty: draft.sleep,
        startAfterCreate: draft.startNow,
        nodeId: draft.nodeId === "local" ? "" : draft.nodeId,
        modpack:
          isPack && pack
            ? { platform: pack.platform, project: pack.project, versionId: pack.versionId }
            : undefined,
      });
      if (!server) throw new Error("The server wasn't created.");
      // Networking is set up after the server exists; a failure here
      // leaves the server in place, reachable the default way.
      try {
        if (draft.network === "domain" && address) {
          await proxyClient.upsertRoute({ hostname: address, serverId: server.id });
        } else if (draft.network === "hostname") {
          await proxyClient.upsertRoute({
            hostname: draft.hostname.trim().toLowerCase(),
            serverId: server.id,
          });
        } else if (draft.network === "port") {
          const { listener } = await proxyClient.upsertListener({ bindAddr: "0.0.0.0", port, enabled: true });
          await proxyClient.upsertRoute({ hostname: "*", serverId: server.id, listenerId: listener?.id });
        }
      } catch (err) {
        toast.error(`${server.name} was created, but its address couldn't be set up: ${errorMessage(err)}`, {
          duration: 15_000,
        });
      }
      return server;
    },
    onSuccess: async (server) => {
      await queryClient.invalidateQueries({ queryKey: ["servers"] });
      void queryClient.invalidateQueries({ queryKey: ["proxy"] });
      onOpenChange(false);
      toast.success(`${server.name} created. ${isPack ? "Installing the modpack…" : "Installing…"}`);
      await navigate({ to: "/servers/$serverId", params: { serverId: server.id } });
    },
  });

  const summary: [string, string][] = [
    [
      "Type",
      isPack && draft.pack ? `${draft.pack.title} (${PLATFORM_NAMES[draft.pack.platform]})` : softwareName,
    ],
    [
      "Version",
      isPack
        ? [
            draft.pack?.versionName,
            draft.pack?.mcVersion && `Minecraft ${draft.pack.mcVersion}`,
            draft.pack?.loader && loaderName(draft.pack.loader),
          ]
            .filter(Boolean)
            .join(" · ") || "Newest"
        : `${draft.mcVersion}${draft.loaderVersion ? ` · ${loaderName(draft.software)} ${draft.loaderVersion}` : ""}`,
    ],
    ...(machines.length > 1 ? ([["Machine", machine?.name ?? "This machine"]] as [string, string][]) : []),
    ["Memory", `${(draft.memoryMb / 1024).toFixed(draft.memoryMb % 1024 ? 1 : 0)} GB`],
    [
      "Address",
      draft.network === "domain"
        ? (address ?? "No domain yet")
        : draft.network === "hostname"
          ? draft.hostname.trim().toLowerCase()
          : draft.network === "port"
            ? `This machine's address, port ${draft.port}`
            : "None yet (add one on Proxy & routes)",
    ],
  ];

  const next = () => {
    if (problem) return;
    if (step < STEPS.length - 1) setStep(step + 1);
    else create.mutate();
  };

  return (
    <>
      <FromTemplateDialog template={template} onClose={() => setTemplate(null)} />
      <Dialog
        open={open}
        onOpenChange={(o) => {
          if (!o) create.reset();
          onOpenChange(o);
        }}
      >
        <DialogContent className="flex w-[620px] flex-col">
          <DialogHeader>
            <DialogTitle>A new world awaits.</DialogTitle>
            <DialogDescription>Choose the basics. Your next server starts here.</DialogDescription>
          </DialogHeader>
          <ol className="mb-5 flex gap-1.5" aria-label="Steps">
            {STEPS.map((name, i) => (
              <li key={name} className="flex-1">
                <button
                  type="button"
                  disabled={i > step}
                  onClick={() => setStep(i)}
                  aria-current={i === step ? "step" : undefined}
                  className="w-full text-left disabled:cursor-default"
                >
                  <span
                    className={cn("block h-1 rounded-full", i <= step ? "bg-primary" : "bg-surface-subtle")}
                    aria-hidden
                  />
                  <span
                    className={cn(
                      "mt-1.5 block text-meta",
                      i === step ? "text-text-primary" : "text-text-muted",
                    )}
                  >
                    {i + 1}. {name}
                  </span>
                </button>
              </li>
            ))}
          </ol>
          <form
            onSubmit={(e) => {
              e.preventDefault();
              next();
            }}
            noValidate
          >
            {create.isError && <ErrorAlert>{errorMessage(create.error)}</ErrorAlert>}
            <div className="min-h-[300px]">
              {step === 0 && (
                <>
                  <SoftwareStep draft={draft} update={update} />
                  <TemplatePicker
                    onUse={(t) => {
                      onOpenChange(false);
                      setTemplate(t);
                    }}
                  />
                </>
              )}
              {step === 1 &&
                (isPack ? (
                  <ModpackStep draft={draft} update={update} />
                ) : (
                  <VersionStep draft={draft} update={update} />
                ))}
              {step === 2 && (
                <ResourcesStep
                  draft={draft}
                  update={update}
                  machines={machines}
                  hostMemory={
                    Number(
                      (draft.nodeId === "local" ? host.data?.memoryBytes : machine?.host?.memoryBytes) ?? 0,
                    ) || undefined
                  }
                />
              )}
              {step === 3 && <NetworkStep draft={draft} update={update} canManageProxy={canManageProxy} />}
              {step === 4 && <ReviewStep draft={draft} update={update} summary={summary} />}
            </div>
            <DialogFooter className="items-center">
              <p className="mr-auto text-meta text-text-muted" aria-live="polite">
                {problem && step > 0 ? problem : `NetherNode ${health.data?.version ?? ""}`.trim()}
              </p>
              {step > 0 ? (
                <Button onClick={() => setStep(step - 1)}>
                  <ArrowLeft />
                  Back
                </Button>
              ) : (
                <Button onClick={() => onOpenChange(false)}>Cancel</Button>
              )}
              <Button type="submit" variant="primary" disabled={!!problem || create.isPending}>
                {step < STEPS.length - 1 ? (
                  <>
                    Next
                    <ArrowRight />
                  </>
                ) : (
                  <>
                    <Plus />
                    {create.isPending ? "Creating…" : "Create server"}
                  </>
                )}
              </Button>
            </DialogFooter>
          </form>
        </DialogContent>
      </Dialog>
    </>
  );
}
