import { useQuery } from "@tanstack/react-query";
import { CircleCheck, ExternalLink, Globe, KeyRound, Network, Plug, TriangleAlert } from "lucide-react";
import { useEffect, useId, useRef, useState } from "react";

import { Checkbox } from "@/components/ui/checkbox";
import { Field, Input, Select } from "@/components/ui/field";
import { Platform, ProjectKind } from "@/gen/nethernode/v1/catalog_pb";
import { errorMessage, proxyClient } from "@/lib/api";
import { cn } from "@/lib/utils";

import { PlatformTabs, ProjectIcon, ProjectSearch } from "../../catalog/project-search";
import {
  curseForgeStatusQuery,
  loaderVersionsQuery,
  mcVersionsQuery,
  projectVersionsQuery,
} from "../../catalog/queries";
import { routesQuery } from "../../proxy/queries";
import { formatBytes, loaderName, memoryLimitBytes } from "../format";
import { THEME_NAMES } from "../world-banner";
import { curseForgeSlug, type Draft, MORE_SOFTWARE, type PackChoice, SOFTWARE, slugPreview } from "./draft";

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
          placeholder="1.21.11, or LATEST"
          onChange={(e) => update({ mcVersion: e.target.value })}
        />
      </div>
    );
  }
  return (
    <div>
      <div className="mb-[17px]">
        <Label htmlFor={ids.mc}>Minecraft version</Label>
        <Select
          id={ids.mc}
          value={draft.mcVersion}
          disabled={versions.isPending}
          onChange={(e) => update({ mcVersion: e.target.value, loaderVersion: "" })}
        >
          {versions.isPending && <option>Loading…</option>}
          {versions.data?.map((v) => (
            <option key={v.id} value={v.id}>
              {v.id}
              {v.snapshot ? " (snapshot)" : ""}
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
            Show snapshots and pre-releases
          </label>
        </div>
      </div>
      {loader !== "vanilla" && (
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

/** Step 2 for modpacks: which pack and version. */
export function ModpackStep({ draft, update }: { draft: Draft; update: Update }) {
  const [platform, setPlatform] = useState(draft.pack?.platform ?? Platform.MODRINTH);
  const curseForgeKey = useQuery(curseForgeStatusQuery);
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
  hostMemory,
}: {
  draft: Draft;
  update: Update;
  hostMemory?: number;
}) {
  const id = useId();
  const max = Math.max(
    4096,
    Math.min(32768, hostMemory ? Math.floor(hostMemory / 1048576 / 512) * 512 : 16384),
  );
  const gb = (mb: number) => `${(mb / 1024).toFixed(mb % 1024 ? 1 : 0)} GB`;
  const recommended = SOFTWARE.find((s) => s.id === draft.software)?.memoryMb ?? 4096;
  const limit = memoryLimitBytes(draft.memoryMb, draft.software === "modpack" ? "modrinth" : draft.software);
  return (
    <div>
      <Label htmlFor={id}>Memory for Minecraft (Java heap): {gb(draft.memoryMb)}</Label>
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
          {draft.software === "modpack"
            ? "a modpack"
            : (SOFTWARE.find((s) => s.id === draft.software)?.name ?? "this")}
          : {gb(recommended)}
          {draft.software === "modpack" ? " to start; big packs want 8 GB or more" : ""}.
        </li>
        <li>
          The container may use up to {formatBytes(limit)} in all, since Java needs room beyond the heap.
        </li>
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
export function NetworkStep({
  draft,
  update,
  canManageProxy,
}: {
  draft: Draft;
  update: Update;
  canManageProxy: boolean;
}) {
  const ids = { host: useId(), port: useId() };
  const routes = useQuery({ ...routesQuery, enabled: canManageProxy });
  const wildcards = (routes.data ?? [])
    .filter((r) => r.hostname.startsWith("*.") && !r.serverId)
    .map((r) => r.hostname.slice(2));
  const slug = slugPreview(draft.name || draft.pack?.title || draft.software);
  const host = draft.hostname.trim().toLowerCase();
  const [dns, setDns] = useState<{ host: string; addresses: string[]; error: string } | null>(null);

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
        <span className="mt-0.5 block text-meta text-text-secondary">Players join with {joinWith}</span>
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
      {option(
        "default",
        <Network />,
        "A name you set up later",
        wildcards.length > 0 ? (
          <>{code(wildcards.map((w) => `${slug}.${w}`).join(" or "))}</>
        ) : (
          "an address you add later"
        ),
        wildcards.length > 0
          ? "Your wildcard route already covers it, using the server's short name. Nothing else to set up."
          : "Nothing is set up now. Until you add a hostname or port on Proxy & routes, players can't join it.",
      )}
      {canManageProxy &&
        option(
          "hostname",
          <Globe />,
          "Its own hostname",
          code(draft.hostname.trim() || "play.example.com"),
          "NetherNode adds a route so that name leads here, on the usual port (25565). You need a DNS record for the name pointing at this machine, or your router if you're at home.",
        )}
      {draft.network === "hostname" && (
        <div className="ml-7">
          <Input
            id={ids.host}
            aria-label="Hostname"
            placeholder="play.example.com"
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
            "Stops after 10 minutes without players; starts again when someone joins.",
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
