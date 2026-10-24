import { useQuery } from "@tanstack/react-query";
import {
  CircleCheck,
  Clock,
  ExternalLink,
  Globe,
  KeyRound,
  Network,
  Plug,
  TriangleAlert,
} from "lucide-react";
import { useEffect, useId, useRef, useState } from "react";
import { Badge } from "@/components/status-badge";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Field, Input, Select } from "@/components/ui/field";
import { proxyClient } from "@/features/proxy/client";
import { Platform, ProjectKind, type ProjectVersion } from "@/gen/nethernode/v1/catalog_pb";
import { type Node, NodeStatus } from "@/gen/nethernode/v1/nodes_pb";
import { errorMessage } from "@/lib/api";
import { dateTime } from "@/lib/time";
import { cn } from "@/lib/utils";

import { PlatformTabs, ProjectIcon, ProjectSearch } from "../../catalog/project-search";
import {
  curseForgeStatusQuery,
  loaderVersionsQuery,
  mcVersionsQuery,
  projectVersionsQuery,
} from "../../catalog/queries";
import { domainError, domainsQuery, useSetDomains } from "../../domains/queries";
import { listenersQuery, routesQuery } from "../../proxy/queries";
import { BEDROCK_FIRST_PORT, nextFreePort } from "../bedrock";
import { formatBytes, loaderName, memoryLimitBytes, versionLabel } from "../format";
import { THEME_NAMES } from "../world-banner";
import {
  curseForgeSlug,
  type Draft,
  domainAddress,
  MORE_SOFTWARE,
  type PackChoice,
  SOFTWARE,
  slugPreview,
  softwareInfo,
  subdomainError,
} from "./draft";

export const EULA_URL = "https://aka.ms/MinecraftEULA";

type Update = (patch: Partial<Draft>) => void;

function Label({ htmlFor, children }: { htmlFor?: string; children: React.ReactNode }) {
  return (
    <label htmlFor={htmlFor} className="mb-[7px] block text-label text-text-secondary">
      {children}
    </label>
  );
}

function Hint({
  children,
  tone = "muted",
}: {
  children: React.ReactNode;
  tone?: "muted" | "danger" | "success";
}) {
  return (
    <p
      className={cn(
        "mt-1.5 text-meta",
        tone === "danger" ? "text-danger" : tone === "success" ? "text-success" : "text-text-muted",
      )}
    >
      {children}
    </p>
  );
}

/** Step 1: what kind of server. */
export function SoftwareStep({ draft, update }: { draft: Draft; update: Update }) {
  const [more, setMore] = useState(MORE_SOFTWARE.some((s) => s.id === draft.software));
  const options = more ? [...SOFTWARE, ...MORE_SOFTWARE] : SOFTWARE;
  return (
    <fieldset>
      <legend className="sr-only">Server type</legend>
      <div className="grid grid-cols-2 gap-2.5 below-sm:grid-cols-1">
        {options.map((s) => (
          <label
            key={s.id}
            className={cn(
              "flex cursor-pointer flex-col rounded-md border px-3.5 py-3",
              draft.software === s.id
                ? "border-primary bg-nav-active"
                : "border-divider hover:border-border-strong",
            )}
          >
            <input
              type="radio"
              name="software"
              value={s.id}
              checked={draft.software === s.id}
              onChange={() =>
                update({
                  software: s.id,
                  memoryMb: s.memoryMb,
                  mcVersion: "",
                  loaderVersion: "",
                  pack: undefined,
                  // Bedrock players connect to a UDP port of its own; the rest use names.
                  ...(s.id === "bedrock"
                    ? { network: "port" as const, port: "" }
                    : draft.software === "bedrock"
                      ? { network: "domain" as const, port: "" }
                      : {}),
                })
              }
              className="sr-only"
            />
            <span className="font-medium text-label">{s.name}</span>
            <span className="text-meta text-text-muted">{s.blurb}</span>
          </label>
        ))}
      </div>
      {!more && (
        <button
          type="button"
          onClick={() => setMore(true)}
          className="mt-2.5 text-meta text-text-secondary underline underline-offset-2 hover:text-text-primary"
        >
          More: Purpur, Quilt
        </button>
      )}
    </fieldset>
  );
}

/** Step 2 for server software: Minecraft version and build. */
export function VersionStep({ draft, update }: { draft: Draft; update: Update }) {
  const ids = { mc: useId(), build: useId(), snapshots: useId() };
  const loader = draft.software;
  const bedrock = loader === "bedrock";
  const versions = useQuery(mcVersionsQuery(loader, draft.snapshots));
  const builds = useQuery(loaderVersionsQuery(loader, draft.mcVersion));

  // Newest release by default.
  useEffect(() => {
    const first = versions.data?.[0];
    if (first && !versions.data?.some((v) => v.id === draft.mcVersion)) update({ mcVersion: first.id });
  }, [versions.data, draft.mcVersion, update]);
  // The recommended build by default, once per Minecraft version.
  const defaulted = useRef("");
  useEffect(() => {
    if (!builds.data || defaulted.current === draft.mcVersion) return;
    defaulted.current = draft.mcVersion;
    update({ loaderVersion: builds.data.find((b) => b.recommended)?.version ?? "" });
  }, [builds.data, draft.mcVersion, update]);

  if (versions.isError) {
    return (
      <div>
        <p className="mb-3 flex items-start gap-2 rounded-md border border-warning/30 bg-warning-subtle px-3 py-2 text-meta text-warning">
          <TriangleAlert className="mt-0.5 size-4 shrink-0" aria-hidden />
          Couldn't load the version list ({errorMessage(versions.error)}). Type a version instead.
        </p>
        <Field
          label="Minecraft version"
          className="[&_input]:font-mono"
          value={draft.mcVersion}
          placeholder={bedrock ? "LATEST, or 1.21.50.07" : "1.21.11, or LATEST"}
          onChange={(e) => update({ mcVersion: e.target.value })}
        />
      </div>
    );
  }
  return (
    <div>
      <div className="mb-[17px]">
        <Label htmlFor={ids.mc}>{bedrock ? "Bedrock version" : "Minecraft version"}</Label>
        <Select
          id={ids.mc}
          value={draft.mcVersion}
          disabled={versions.isPending}
          onChange={(e) => update({ mcVersion: e.target.value, loaderVersion: "" })}
        >
          {versions.isPending && <option>Loading…</option>}
          {versions.data?.map((v) => (
            <option key={v.id} value={v.id}>
              {bedrock ? versionLabel(v.id) : v.id}
              {v.snapshot ? (bedrock ? " (beta)" : " (snapshot)") : ""}
            </option>
          ))}
        </Select>
        <div className="mt-2 flex items-center gap-2">
          <Checkbox
            id={ids.snapshots}
            checked={draft.snapshots}
            onCheckedChange={(v) => update({ snapshots: v === true })}
          />
          <label htmlFor={ids.snapshots} className="text-meta text-text-secondary">
            {bedrock ? "Show the preview (beta) version" : "Show snapshots and pre-releases"}
          </label>
        </div>
        {bedrock && (
          <Hint>
            Bedrock apps update on their own and must match the server. Latest follows them each time the
            server restarts.
          </Hint>
        )}
      </div>
      {loader !== "vanilla" && !bedrock && (
        <div className="mb-[17px]">
          <Label htmlFor={ids.build}>{loaderName(loader)} build</Label>
          {builds.isError ? (
            <Hint tone="danger">
              Couldn't load builds: {errorMessage(builds.error)}. The newest will be used.
            </Hint>
          ) : builds.data && builds.data.length === 0 ? (
            <Hint tone="danger">
              {loaderName(loader)} has no builds for Minecraft {draft.mcVersion}. Pick another version.
            </Hint>
          ) : (
            <>
              <Select
                id={ids.build}
                value={draft.loaderVersion}
                disabled={builds.isPending}
                onChange={(e) => update({ loaderVersion: e.target.value })}
              >
                <option value="">Newest (updated when the server restarts)</option>
                {builds.data?.slice(0, 60).map((b) => (
                  <option key={b.version} value={b.version}>
                    {b.version}
                    {b.recommended ? " (recommended)" : b.stable ? "" : " (beta)"}
                  </option>
                ))}
              </Select>
              <Hint>Pinning a build keeps the server on it until you change it.</Hint>
            </>
          )}
        </div>
      )}
    </div>
  );
}

/** Step 2 for modpacks: which pack and version. With `fixed` (a pack chosen
 * on the Modpacks page), only that pack's versions. */
export function ModpackStep({ draft, update, fixed }: { draft: Draft; update: Update; fixed?: boolean }) {
  const [platform, setPlatform] = useState(draft.pack?.platform ?? Platform.MODRINTH);
  const curseForgeKey = useQuery(curseForgeStatusQuery);
  if (fixed && draft.pack?.project) {
    return <ChosenPack pack={draft.pack} onChange={(p) => update({ pack: p })} />;
  }
  const pack = draft.pack?.platform === platform ? draft.pack : undefined;
  const choose = (p: PackChoice) => update({ pack: p, name: draft.name || p.title.slice(0, 40) });
  const keyless = platform === Platform.CURSEFORGE && curseForgeKey.data === false;

  return (
    <div>
      <div className="mb-3 flex items-center justify-between gap-2">
        <PlatformTabs value={platform} onChange={setPlatform} />
        {pack && (
          <span className="flex min-w-0 items-center gap-2 text-meta text-text-secondary">
            <ProjectIcon url={pack.iconUrl} size={20} />
            <span className="truncate">{pack.title}</span>
          </span>
        )}
      </div>
      {keyless ? (
        <KeylessCurseForge draft={draft} update={update} />
      ) : (
        <>
          <ProjectSearch
            platform={platform}
            kind={ProjectKind.MODPACK}
            selectedId={pack?.project}
            onSelect={(p) =>
              choose({
                platform,
                project: p.id,
                title: p.title,
                iconUrl: p.iconUrl,
                versionId: "",
                versionName: "",
                mcVersion: "",
                loader: p.loaders[0] ?? "",
              })
            }
          />
          {pack && <PackVersion pack={pack} onChange={(p) => update({ pack: p })} />}
        </>
      )}
    </div>
  );
}

/** The pack chosen on the Modpacks page, and its versions to pick from. */
function ChosenPack({ pack, onChange }: { pack: PackChoice; onChange: (pack: PackChoice) => void }) {
  const versions = useQuery(projectVersionsQuery(pack.platform, pack.project));
  const choose = (v: ProjectVersion) =>
    onChange({
      ...pack,
      versionId: v.id,
      versionName: v.versionNumber,
      mcVersion: v.gameVersions[v.gameVersions.length - 1] ?? "",
      loader: v.loaders[0] ?? pack.loader,
    });
  // The newest release to start with.
  useEffect(() => {
    if (!versions.data || pack.versionId) return;
    const pick = versions.data.find((v) => v.channel === "release") ?? versions.data[0];
    if (pick)
      onChange({
        ...pack,
        versionId: pick.id,
        versionName: pick.versionNumber,
        mcVersion: pick.gameVersions[pick.gameVersions.length - 1] ?? "",
        loader: pick.loaders[0] ?? pack.loader,
      });
  }, [versions.data, pack, onChange]);
  const list = versions.data?.slice(0, 50) ?? [];

  return (
    <div>
      <div className="mb-4 flex items-center gap-3 rounded-lg border border-border px-3.5 py-3">
        <ProjectIcon url={pack.iconUrl} size={40} />
        <div className="min-w-0">
          <p className="truncate font-medium text-label">{pack.title}</p>
          <p className="text-meta text-text-muted">
            {pack.platform === Platform.CURSEFORGE ? "CurseForge" : "Modrinth"} modpack
          </p>
        </div>
      </div>
      {versions.isError ? (
        <Hint tone="danger">{errorMessage(versions.error)}</Hint>
      ) : versions.isPending ? (
        <div className="h-48 animate-pulse rounded-lg bg-surface-subtle" aria-busy />
      ) : list.length === 0 ? (
        <Hint tone="danger">{pack.title} has no versions to install.</Hint>
      ) : (
        <fieldset>
          <legend className="mb-[7px] text-label text-text-secondary">Version</legend>
          <div className="max-h-[340px] divide-y divide-divider overflow-y-auto rounded-lg border border-border">
            {list.map((v) => {
              const chosen = v.id === pack.versionId;
              return (
                <label
                  key={v.id}
                  className={cn(
                    "flex cursor-pointer items-center gap-3 px-3.5 py-2.5 transition-colors has-[:focus-visible]:outline-2 has-[:focus-visible]:outline-primary has-[:focus-visible]:-outline-offset-2",
                    chosen ? "bg-primary-subtle" : "hover:bg-surface-subtle",
                  )}
                >
                  <input
                    type="radio"
                    name={`pack-version-${pack.project}`}
                    value={v.id}
                    checked={chosen}
                    onChange={() => choose(v)}
                    className="sr-only"
                  />
                  <span
                    aria-hidden
                    className={cn(
                      "grid size-4 shrink-0 place-items-center rounded-full border",
                      chosen ? "border-primary" : "border-border-strong",
                    )}
                  >
                    {chosen && <span className="size-2 rounded-full bg-primary" />}
                  </span>
                  <span className="min-w-0 flex-1">
                    <span className="flex flex-wrap items-center gap-x-2 gap-y-1">
                      <span className="truncate font-medium text-label">{v.versionNumber || v.name}</span>
                      {v.channel !== "release" && (
                        <Badge tone="warning" icon={<TriangleAlert className="size-3" />}>
                          {v.channel === "beta" ? "Beta" : "Alpha"}
                        </Badge>
                      )}
                    </span>
                    <span className="block text-meta text-text-muted">
                      {[
                        v.gameVersions.length > 0 && `Minecraft ${v.gameVersions.join(", ")}`,
                        v.loaders.length > 0 && v.loaders.map(loaderName).join(", "),
                        v.publishedMs ? dateTime(v.publishedMs) : "",
                      ]
                        .filter(Boolean)
                        .join(" · ")}
                    </span>
                  </span>
                </label>
              );
            })}
          </div>
        </fieldset>
      )}
      {pack.mcVersion && (
        <Hint>
          Runs Minecraft {pack.mcVersion}
          {pack.loader ? ` with ${loaderName(pack.loader)}` : ""}.
        </Hint>
      )}
    </div>
  );
}

function PackVersion({ pack, onChange }: { pack: PackChoice; onChange: (pack: PackChoice) => void }) {
  const id = useId();
  const versions = useQuery(projectVersionsQuery(pack.platform, pack.project));
  useEffect(() => {
    if (!versions.data || pack.versionId) return;
    const pick = versions.data.find((v) => v.channel === "release") ?? versions.data[0];
    if (pick)
      onChange({
        ...pack,
        versionId: pick.id,
        versionName: pick.versionNumber,
        mcVersion: pick.gameVersions[pick.gameVersions.length - 1] ?? "",
        loader: pick.loaders[0] ?? pack.loader,
      });
  }, [versions.data, pack, onChange]);
  return (
    <div className="mt-4 rounded-md border border-divider p-3">
      <Label htmlFor={id}>{pack.title} version</Label>
      {versions.isError ? (
        <Hint tone="danger">{errorMessage(versions.error)}</Hint>
      ) : (
        <Select
          id={id}
          value={pack.versionId}
          disabled={versions.isPending}
          onChange={(e) => {
            const v = versions.data?.find((x) => x.id === e.target.value);
            if (v)
              onChange({
                ...pack,
                versionId: v.id,
                versionName: v.versionNumber,
                mcVersion: v.gameVersions[v.gameVersions.length - 1] ?? "",
                loader: v.loaders[0] ?? pack.loader,
              });
          }}
        >
          {versions.isPending && <option>Loading…</option>}
          {versions.data?.slice(0, 50).map((v) => (
            <option key={v.id} value={v.id}>
              {v.versionNumber} · Minecraft {v.gameVersions.join(", ")}
              {v.channel !== "release" ? ` (${v.channel})` : ""}
            </option>
          ))}
        </Select>
      )}
      {pack.mcVersion && (
        <Hint>
          Runs Minecraft {pack.mcVersion}
          {pack.loader ? ` with ${loaderName(pack.loader)}` : ""}.
        </Hint>
      )}
    </div>
  );
}

/** CurseForge without an API key: a slug and the Minecraft version. */
function KeylessCurseForge({ draft, update }: { draft: Draft; update: Update }) {
  const ids = { slug: useId(), mc: useId() };
  const versions = useQuery(mcVersionsQuery("forge", false));
  const pack = draft.pack?.platform === Platform.CURSEFORGE ? draft.pack : undefined;
  const set = (patch: Partial<PackChoice>) => {
    const next: PackChoice = {
      platform: Platform.CURSEFORGE,
      project: "",
      title: "",
      iconUrl: "",
      versionId: "",
      versionName: "",
      mcVersion: "",
      loader: "",
      ...pack,
      ...patch,
    };
    next.title = next.project.replace(/[-_]+/g, " ");
    update({ pack: next, name: draft.name || next.title.slice(0, 40) });
  };
  return (
    <div>
      <p className="mb-3 flex items-start gap-2 rounded-md border border-divider bg-surface-subtle px-3 py-2 text-meta text-text-secondary">
        <KeyRound className="mt-0.5 size-4 shrink-0 text-text-muted" aria-hidden />
        <span>
          Searching CurseForge needs an API key (add one in Settings). Without one, paste the pack's page
          address and choose its Minecraft version; the server still installs it.
        </span>
      </p>
      <Field
        label="Modpack page or slug"
        id={ids.slug}
        placeholder="https://www.curseforge.com/minecraft/modpacks/…"
        value={pack?.project ?? ""}
        onChange={(e) => set({ project: curseForgeSlug(e.target.value) })}
      />
      <div className="mb-[17px]">
        <Label htmlFor={ids.mc}>The pack's Minecraft version</Label>
        <Select
          id={ids.mc}
          value={pack?.mcVersion ?? ""}
          onChange={(e) => set({ mcVersion: e.target.value })}
        >
          <option value="">Choose…</option>
          {versions.data?.map((v) => (
            <option key={v.id} value={v.id}>
              {v.id}
            </option>
          ))}
        </Select>
        <Hint>It decides which Java the server runs. It's on the pack's page.</Hint>
      </div>
    </div>
  );
}

/** Step 3: memory. */
export function ResourcesStep({
  draft,
  update,
  machines,
  hostMemory,
}: {
  draft: Draft;
  update: Update;
  machines: Node[];
  hostMemory?: number;
}) {
  const id = useId();
  const machineId = useId();
  const max = Math.max(
    4096,
    Math.min(65536, hostMemory ? Math.floor(hostMemory / 1048576 / 512) * 512 : 16384),
  );
  const gb = (mb: number) => `${(mb / 1024).toFixed(mb % 1024 ? 1 : 0)} GB`;
  const recommended = softwareInfo(draft.software)?.memoryMb ?? 4096;
  const bedrock = draft.software === "bedrock";
  const limit = memoryLimitBytes(draft.memoryMb, draft.software === "modpack" ? "modrinth" : draft.software);
  return (
    <div>
      {machines.length > 1 && (
        <div className="mb-5">
          <Label htmlFor={machineId}>Machine</Label>
          <Select id={machineId} value={draft.nodeId} onChange={(e) => update({ nodeId: e.target.value })}>
            {machines.map((m) => (
              <option key={m.id} value={m.id} disabled={m.status !== NodeStatus.ONLINE || m.updateNeeded}>
                {m.name}
                {m.status !== NodeStatus.ONLINE ? " (offline)" : m.updateNeeded ? " (needs an update)" : ""}
              </option>
            ))}
          </Select>
          <Hint>Players still join through this panel, whichever machine runs the server.</Hint>
        </div>
      )}
      <Label htmlFor={id}>
        {bedrock ? "Memory for the server" : "Memory for Minecraft (Java heap)"}: {gb(draft.memoryMb)}
      </Label>
      <input
        id={id}
        type="range"
        min={1024}
        max={max}
        step={512}
        value={draft.memoryMb}
        onChange={(e) => update({ memoryMb: Number(e.target.value) })}
        className="w-full accent-primary"
        aria-valuetext={gb(draft.memoryMb)}
      />
      <div className="flex justify-between text-meta text-text-muted">
        <span>1 GB</span>
        <span>{gb(max)}</span>
      </div>
      <ul className="mt-4 flex flex-col gap-1.5 text-meta text-text-secondary">
        <li>
          Suggested for{" "}
          {draft.software === "modpack" ? "a modpack" : (softwareInfo(draft.software)?.name ?? "this")}:{" "}
          {gb(recommended)}
          {draft.software === "modpack" ? " to start; big packs want 8 GB or more" : ""}.
        </li>
        <li>
          {bedrock
            ? `The server can use up to ${formatBytes(limit)}. Bedrock is light: 2 GB suits a few friends.`
            : `The container may use up to ${formatBytes(limit)} in all, since Java needs room beyond the heap.`}
        </li>
        {bedrock && (
          <li>
            Mojang builds the Bedrock server for x86-64 only; on an ARM machine it runs under emulation.
          </li>
        )}
        {hostMemory ? (
          <li className={cn(limit > hostMemory * 0.8 && "text-warning")}>
            This host has {formatBytes(hostMemory)} of memory, shared with your other servers.
          </li>
        ) : null}
      </ul>
    </div>
  );
}

/** Step 4: how players reach it. */
export function NetworkStep(props: { draft: Draft; update: Update; canManageProxy: boolean }) {
  return props.draft.software === "bedrock" ? <BedrockNetwork {...props} /> : <JavaNetworkStep {...props} />;
}

/** Bedrock players connect over UDP to a port of the server's own. */
function BedrockNetwork({
  draft,
  update,
  canManageProxy,
}: {
  draft: Draft;
  update: Update;
  canManageProxy: boolean;
}) {
  const id = useId();
  const listeners = useQuery({ ...listenersQuery, enabled: canManageProxy });
  const used = listeners.data;
  // The lowest free port to start with, once the listeners are known.
  useEffect(() => {
    if (canManageProxy && !draft.port && used) update({ port: String(nextFreePort(used)) });
    // Only admins can open ports; for others the address comes later.
    if (!canManageProxy && draft.network !== "later") update({ network: "later" });
  }, [canManageProxy, draft.port, draft.network, used, update]);
  const port = Number(draft.port);
  const clash = used?.some((l) => l.port === port);

  if (!canManageProxy) {
    return (
      <p className="text-label text-text-secondary">
        An admin opens the port players use for this server once it's created.
      </p>
    );
  }
  return (
    <div>
      <Label htmlFor={id}>Port players connect to (UDP)</Label>
      <Input
        id={id}
        inputMode="numeric"
        placeholder={String(BEDROCK_FIRST_PORT)}
        className="w-32 font-mono"
        aria-invalid={clash ? true : undefined}
        value={draft.port}
        onChange={(e) => update({ port: e.target.value.replace(/\D/g, "").slice(0, 5) })}
      />
      {clash ? (
        <Hint tone="danger">Another listener already uses port {port}.</Hint>
      ) : (
        <Hint>Each Bedrock server has a port of its own; 19132 is the one Minecraft fills in for you.</Hint>
      )}
      <ul className="mt-4 flex flex-col gap-1.5 text-meta text-text-secondary">
        <li>
          Players add the server in Minecraft: Play → Servers → Add Server, with this machine's address and
          that port.
        </li>
        <li>The port is UDP. Forward it on your router for friends outside your network.</li>
        <li>
          A sleeping server wakes when someone tries to join; they'll see "unable to connect" once and can
          retry in a moment.
        </li>
        <li>Xbox, PlayStation and Switch can't add servers by address.</li>
      </ul>
    </div>
  );
}

function JavaNetworkStep({
  draft,
  update,
  canManageProxy,
}: {
  draft: Draft;
  update: Update;
  canManageProxy: boolean;
}) {
  const ids = { host: useId(), port: useId(), sub: useId(), domain: useId(), apex: useId(), add: useId() };
  const domainsList = useQuery({ ...domainsQuery, enabled: canManageProxy });
  const domains = domainsList.data ?? [];
  const routes = useQuery({ ...routesQuery, enabled: canManageProxy });
  const address = domainAddress(draft, domains);
  // The dialog explains a name that's already taken.
  const taken = address ? routes.data?.find((r) => r.hostname === address) : undefined;
  const subProblem = subdomainError(draft.subdomain);
  const namePreview = slugPreview(draft.name || draft.pack?.title || "server");
  const host = draft.hostname.trim().toLowerCase();
  const [dns, setDns] = useState<{ host: string; addresses: string[]; error: string } | null>(null);
  const [newDomain, setNewDomain] = useState("");
  const addDomain = useSetDomains();
  const newDomainProblem = newDomain ? domainError(newDomain) : undefined;

  // Look the name up as it's typed (debounced).
  useEffect(() => {
    if (draft.network !== "hostname" || !/^[a-z0-9.-]+\.[a-z]{2,}$/.test(host)) return;
    const timer = setTimeout(() => {
      proxyClient
        .checkDns({ hostname: host })
        .then((r) => setDns({ host, addresses: r.addresses, error: r.error }))
        .catch((e) => setDns({ host, addresses: [], error: errorMessage(e) }));
    }, 500);
    return () => clearTimeout(timer);
  }, [draft.network, host]);

  const option = (
    value: Draft["network"],
    icon: React.ReactNode,
    title: string,
    joinWith: React.ReactNode,
    body: React.ReactNode,
  ) => (
    <label
      className={cn(
        "flex cursor-pointer items-start gap-3 rounded-md border px-3.5 py-3",
        draft.network === value
          ? "border-primary bg-nav-active"
          : "border-divider hover:border-border-strong",
      )}
    >
      <input
        type="radio"
        name="network"
        checked={draft.network === value}
        onChange={() => update({ network: value })}
        className="mt-1 accent-primary"
      />
      <span className="mt-0.5 text-text-muted [&_svg]:size-4" aria-hidden>
        {icon}
      </span>
      <span className="min-w-0">
        <span className="block font-medium text-label">{title}</span>
        {joinWith && (
          <span className="mt-0.5 block text-meta text-text-secondary">Players join with {joinWith}</span>
        )}
        <span className="mt-0.5 block text-meta text-text-muted">{body}</span>
      </span>
    </label>
  );
  const code = (text: string) => <span className="font-mono text-text-primary">{text}</span>;

  return (
    <fieldset className="flex flex-col gap-2.5">
      <legend className="mb-1 text-label text-text-secondary">
        How will players reach this server? Every option goes through NetherNode's proxy, so the server itself
        is never exposed directly.
      </legend>
      {canManageProxy &&
        option(
          "domain",
          <Globe />,
          "A name on your domain",
          address
            ? code(
                draft.apex || draft.subdomain.trim() || draft.name.trim() || draft.pack?.title
                  ? address
                  : `<server name>.${draft.domain || domains[0]}`,
              )
            : null,
          domains.length > 0
            ? "NetherNode adds a route for this name. Your domain's wildcard DNS record already covers it."
            : "Add a domain you own and each server can have its own name on it, like smp.example.com.",
        )}
      {draft.network === "domain" &&
        canManageProxy &&
        (domains.length > 0 ? (
          <div className="ml-7 flex flex-col gap-2">
            <div className="flex items-center gap-1.5">
              <label htmlFor={ids.sub} className="sr-only">
                Name before the domain
              </label>
              <Input
                id={ids.sub}
                placeholder={draft.name.trim() || draft.pack?.title ? namePreview : "server name"}
                autoCapitalize="none"
                spellCheck={false}
                className="w-48 font-mono"
                disabled={draft.apex}
                aria-invalid={subProblem ? true : undefined}
                value={draft.apex ? "" : draft.subdomain}
                onChange={(e) => update({ subdomain: e.target.value })}
              />
              <span className="font-mono text-text-muted">.</span>
              {domains.length > 1 ? (
                <>
                  <label htmlFor={ids.domain} className="sr-only">
                    Domain
                  </label>
                  <Select
                    id={ids.domain}
                    className="font-mono"
                    value={draft.domain || domains[0]}
                    onChange={(e) => update({ domain: e.target.value })}
                  >
                    {domains.map((d) => (
                      <option key={d} value={d}>
                        {d}
                      </option>
                    ))}
                  </Select>
                </>
              ) : (
                <span className="font-mono text-label">{domains[0]}</span>
              )}
            </div>
            <div className="flex items-center gap-2">
              <Checkbox
                id={ids.apex}
                checked={draft.apex}
                onCheckedChange={(v) => update({ apex: v === true })}
              />
              <label htmlFor={ids.apex} className="text-meta text-text-secondary">
                Use {draft.domain || domains[0]} itself, with nothing in front
              </label>
            </div>
            {subProblem ? (
              <Hint tone="danger">{subProblem}</Hint>
            ) : taken ? null : (
              !draft.apex &&
              !draft.subdomain.trim() && <Hint>Blank uses the server's name, which you choose next.</Hint>
            )}
          </div>
        ) : (
          <div className="ml-7">
            <div className="flex gap-2">
              <label htmlFor={ids.add} className="sr-only">
                Your domain
              </label>
              <Input
                id={ids.add}
                placeholder="example.com"
                autoCapitalize="none"
                spellCheck={false}
                className="font-mono"
                value={newDomain}
                onChange={(e) => setNewDomain(e.target.value)}
                onKeyDown={(e) => {
                  // Enter adds the domain rather than moving the wizard on.
                  if (e.key === "Enter") {
                    e.preventDefault();
                    if (!domainError(newDomain)) addDomain.mutate([newDomain]);
                  }
                }}
              />
              <Button
                disabled={!newDomain.trim() || addDomain.isPending}
                onClick={() => {
                  if (!domainError(newDomain)) addDomain.mutate([newDomain]);
                }}
              >
                Add domain
              </Button>
            </div>
            <Hint tone={newDomainProblem || addDomain.isError ? "danger" : undefined}>
              {addDomain.isError
                ? errorMessage(addDomain.error)
                : (newDomainProblem ??
                  "Then add an A record for *.yourdomain in your DNS, pointing at this machine. Manage domains in Settings.")}
            </Hint>
          </div>
        ))}
      {canManageProxy &&
        option(
          "hostname",
          <Network />,
          "A different hostname",
          code(host || "play.example.org"),
          "Any full name you control. NetherNode adds a route for it; you need a DNS record for that exact name pointing at this machine (or your router).",
        )}
      {draft.network === "hostname" && (
        <div className="ml-7">
          <Input
            id={ids.host}
            aria-label="Hostname"
            placeholder="play.example.org"
            className="font-mono"
            value={draft.hostname}
            onChange={(e) => update({ hostname: e.target.value })}
          />
          {dns?.host === host &&
            (dns.error ? (
              <Hint tone="danger">
                {host} doesn't resolve yet ({dns.error}). Players can't join until it does.
              </Hint>
            ) : (
              <Hint tone="success">
                <CircleCheck className="mr-1 inline size-3.5 align-[-2px]" aria-hidden />
                {host} points to {dns.addresses.join(", ")}. Make sure that's this machine (or your router).
              </Hint>
            ))}
        </div>
      )}
      {canManageProxy &&
        option(
          "port",
          <Plug />,
          "Its own port",
          code(`<this machine's address>:${draft.port || "25566"}`),
          "NetherNode opens that port and sends everyone who connects to it here, whatever address they typed. Good without a domain; forward the port on your router so friends outside your network can join.",
        )}
      {draft.network === "port" && (
        <div className="ml-7">
          <Input
            id={ids.port}
            aria-label="Port"
            inputMode="numeric"
            placeholder="25566"
            className="w-32 font-mono"
            value={draft.port}
            onChange={(e) => update({ port: e.target.value.replace(/\D/g, "").slice(0, 5) })}
          />
          <Hint>Any free port from 1024 to 65535 other than the proxy's own (usually 25565).</Hint>
        </div>
      )}
      {option(
        "later",
        <Clock />,
        "No address yet",
        null,
        "Nothing is set up now. Players can't join until you give it a name or port on Proxy & routes.",
      )}
    </fieldset>
  );
}

/** Step 5: name, extras, and the EULA. */
export function ReviewStep({
  draft,
  update,
  summary,
}: {
  draft: Draft;
  update: Update;
  summary: [string, string][];
}) {
  const ids = { theme: useId(), sleep: useId(), start: useId(), eula: useId() };
  const bedrock = draft.software === "bedrock";
  return (
    <div>
      <Field
        label="Server name"
        placeholder="My awesome world"
        maxLength={40}
        value={draft.name}
        onChange={(e) => update({ name: e.target.value })}
        autoFocus
      />
      <dl className="mb-[17px] rounded-md border border-divider px-3 text-meta">
        {summary.map(([k, v]) => (
          <div key={k} className="flex justify-between gap-4 border-divider border-b py-2 last:border-b-0">
            <dt className="text-text-muted">{k}</dt>
            <dd className="truncate text-right">{v}</dd>
          </div>
        ))}
      </dl>
      <div className="mb-[17px]">
        <Label htmlFor={ids.theme}>Card theme</Label>
        <Select
          id={ids.theme}
          value={draft.theme}
          onChange={(e) => update({ theme: Number(e.target.value) })}
        >
          {Object.entries(THEME_NAMES).map(([value, label]) => (
            <option key={value} value={value}>
              {label}
            </option>
          ))}
        </Select>
        <Hint>Only changes the card artwork, never the world.</Hint>
      </div>
      {(
        [
          [
            ids.sleep,
            draft.sleep,
            (v: boolean) => update({ sleep: v }),
            "Sleep when empty",
            bedrock
              ? "Stops after 10 minutes without players; starts when someone tries to join (they retry once)."
              : "Stops after 10 minutes without players; starts again when someone joins.",
          ],
          [
            ids.start,
            draft.startNow,
            (v: boolean) => update({ startNow: v }),
            "Start when ready",
            "Start as soon as it's installed.",
          ],
        ] as const
      ).map(([id, checked, set, title, body]) => (
        <div key={id} className="mb-3 flex items-start gap-3">
          <Checkbox id={id} checked={checked} onCheckedChange={(v) => set(v === true)} className="mt-0.5" />
          <label htmlFor={id} className="text-label">
            <span className="block font-[550]">{title}</span>
            <span className="text-meta text-text-muted">{body}</span>
          </label>
        </div>
      ))}
      <div className="mb-1 flex items-start gap-3">
        <Checkbox
          id={ids.eula}
          checked={draft.eula}
          onCheckedChange={(v) => update({ eula: v === true })}
          className="mt-0.5"
        />
        <div className="text-label">
          <label htmlFor={ids.eula} className="block font-[550]">
            I accept the Minecraft EULA
          </label>
          <span className="text-meta text-text-muted">
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
          </span>
        </div>
      </div>
    </div>
  );
}
