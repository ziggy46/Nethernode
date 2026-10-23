import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { ArrowUpCircle, CircleCheck, ExternalLink, RefreshCw, TriangleAlert } from "lucide-react";
import { useState } from "react";

import { ConfirmDialog } from "@/components/confirm-dialog";
import { Badge } from "@/components/status-badge";
import { Button } from "@/components/ui/button";
import { Select } from "@/components/ui/field";
import { Platform, type ProjectVersion } from "@/gen/nethernode/v1/catalog_pb";
import { type Server, ServerStatus } from "@/gen/nethernode/v1/servers_pb";
import { errorMessage } from "@/lib/api";
import { trackJob } from "@/lib/jobs";
import { dateTime } from "@/lib/time";
import { cn } from "@/lib/utils";

import { modClient } from "../catalog/api";
import { ProjectIcon } from "../catalog/project-search";
import { minecraftUpgrade, newestMinecraft, versionPage } from "./modpack";

/** Under the mods list's key, so finished jobs refresh it too. */
const packKey = (serverId: string) => ["mods", serverId, "modpack"] as const;

/** A modpack server's pack: the version installed, newer ones, and updating. */
export function ModpackCard({ server }: { server: Server }) {
  const client = useQueryClient();
  const pack = server.modpack;
  const check = useQuery({
    queryKey: packKey(server.id),
    queryFn: () => modClient.checkModpackUpdate({ serverId: server.id }),
    staleTime: 10 * 60_000,
    retry: false,
  });
  const [chosen, setChosen] = useState<ProjectVersion | null>(null);
  // With players online, they're warned in chat first by default.
  const online = server.status === ServerStatus.RUNNING ? (server.playersOnline ?? 0) : 0;
  const [warnMinutes, setWarnMinutes] = useState(0);
  const update = useMutation({
    mutationFn: (version: ProjectVersion) =>
      modClient.updateModpack({ serverId: server.id, versionId: version.id, warnMinutes }),
    onSuccess: ({ jobId }, version) => {
      setChosen(null);
      trackJob(jobId, `Updating ${pack?.title ?? "the pack"} to ${version.versionNumber}…`);
      void client.invalidateQueries({ queryKey: packKey(server.id) });
    },
  });
  if (!pack) return null;

  const data = check.data;
  const installed = data?.current?.versionNumber || pack.versionName || pack.versionId;
  const status = check.isPending
    ? "Looking for newer versions…"
    : check.isError
      ? errorMessage(check.error)
      : data?.followsNewest
        ? "Not pinned to a version: the newest installs each time it starts."
        : data?.unavailable
          ? `${installed} installed. ${data.unavailable}`
          : !data?.current
            ? `${installed} installed (the platform no longer lists it).`
            : data.newer.length > 0
              ? `${installed} installed · ${data.newer.length} newer version${data.newer.length === 1 ? "" : "s"}`
              : `${installed} installed · up to date`;
  const upgrade = chosen ? minecraftUpgrade(server.mcVersion, chosen.gameVersions) : undefined;

  return (
    <section aria-label="Modpack" className="mb-5 rounded-lg border border-border">
      <div className="flex items-center gap-3 px-3.5 py-3">
        <ProjectIcon size={40} url={pack.iconUrl} />
        <div className="min-w-0 flex-1">
          <p className="flex flex-wrap items-center gap-x-2 font-medium text-label">
            {pack.title}
            {pack.pageUrl && (
              <a
                href={pack.pageUrl}
                target="_blank"
                rel="noreferrer noopener"
                className="inline-flex items-center gap-1 font-normal text-meta text-text-muted underline underline-offset-2"
              >
                Pack page
                <ExternalLink className="size-3" aria-hidden />
              </a>
            )}
          </p>
          <p
            className={cn("text-meta", check.isError ? "text-danger" : "text-text-muted")}
            aria-live="polite"
          >
            {status}
          </p>
          <InstalledFrom pack={pack} />
        </div>
        <Button size="mini" onClick={() => check.refetch()} disabled={check.isFetching}>
          <RefreshCw className={cn(check.isFetching && "animate-spin")} />
          Check for updates
        </Button>
      </div>
      {data && data.newer.length > 0 && (
        <ul className="divide-y divide-divider border-divider border-t" aria-label="Newer versions">
          {data.newer.map((v, i) => {
            const page = versionPage(pack.platform, pack.pageUrl, v.id);
            const mc = newestMinecraft(v.gameVersions);
            return (
              <li key={v.id} className="flex items-center gap-3 px-3.5 py-2.5">
                <div className="min-w-0 flex-1">
                  <p className="flex flex-wrap items-center gap-x-2 gap-y-1">
                    <span className="truncate font-medium text-label">{v.versionNumber || v.name}</span>
                    {i === 0 && (
                      <Badge tone="info" icon={<ArrowUpCircle className="size-3" />}>
                        Newest
                      </Badge>
                    )}
                    {v.channel && v.channel !== "release" && (
                      <Badge tone="warning" icon={<TriangleAlert className="size-3" />}>
                        {v.channel === "beta" ? "Beta" : "Alpha"}
                      </Badge>
                    )}
                  </p>
                  <p className="text-meta text-text-muted">
                    {[mc && `Minecraft ${mc}`, v.publishedMs ? dateTime(v.publishedMs) : ""]
                      .filter(Boolean)
                      .join(" · ")}
                    {page && (
                      <>
                        {" · "}
                        <a
                          href={page}
                          target="_blank"
                          rel="noreferrer noopener"
                          className="inline-flex items-center gap-1 underline underline-offset-2"
                        >
                          Changelog
                          <ExternalLink className="size-3" aria-hidden />
                        </a>
                      </>
                    )}
                  </p>
                </div>
                <Button
                  size="mini"
                  variant={i === 0 ? "primary" : "default"}
                  disabled={update.isPending}
                  onClick={() => {
                    update.reset();
                    setWarnMinutes(online > 0 ? 1 : 0);
                    setChosen(v);
                  }}
                >
                  Update
                </Button>
              </li>
            );
          })}
        </ul>
      )}
      {data?.current && data.newer.length === 0 && !data.unavailable && !check.isFetching && (
        <p className="flex items-center gap-1.5 border-divider border-t px-3.5 py-2 text-meta text-text-muted">
          <CircleCheck className="size-3.5 text-success" aria-hidden />
          No newer version for this server's Minecraft version.
        </p>
      )}
      <ConfirmDialog
        open={chosen !== null}
        onOpenChange={(open) => !open && setChosen(null)}
        title={`Update ${pack.title} to ${chosen?.versionNumber ?? ""}?`}
        description={[
          `${server.name} stops if it's running (players are disconnected), a backup is made, and the new version installs as it starts.`,
          upgrade
            ? `This version moves it from Minecraft ${server.mcVersion} to ${upgrade}; the world can't go back to ${server.mcVersion} except by restoring the backup.`
            : "",
          "The pack's own config files are replaced with the new version's; the backup keeps yours. To go back, restore that backup.",
        ]
          .filter(Boolean)
          .join(" ")}
        confirmLabel={update.isPending ? "Starting…" : "Update"}
        pending={update.isPending}
        error={update.error ? errorMessage(update.error) : undefined}
        onConfirm={() => chosen && update.mutate(chosen)}
      >
        {online > 0 && (
          <div className="mb-4 flex flex-wrap items-center gap-2 text-label text-text-secondary">
            <label htmlFor={`${server.id}-update-warn`}>
              {online} player{online === 1 ? " is" : "s are"} online. Warn them first:
            </label>
            <Select
              id={`${server.id}-update-warn`}
              className="h-8 w-auto"
              value={warnMinutes}
              onChange={(e) => setWarnMinutes(Number(e.target.value))}
            >
              <option value={0}>No, update now</option>
              <option value={1}>1 minute</option>
              <option value={5}>5 minutes</option>
            </Select>
          </div>
        )}
      </ConfirmDialog>
    </section>
  );
}

/** Where a CurseForge server's files come from: the author's server pack, or
 * the modpack itself. */
function InstalledFrom({ pack }: { pack: NonNullable<Server["modpack"]> }) {
  if (pack.platform !== Platform.CURSEFORGE) return null;
  const loader = pack.loaderVersion ? ` · ${loaderName(pack.loader)} ${pack.loaderVersion}` : "";
  return pack.serverPack ? (
    <p className="text-meta text-text-muted">
      From the server pack <span className="font-mono">{pack.serverPack}</span>
      {loader}
    </p>
  ) : (
    <p className="text-meta text-text-muted">
      From the modpack, leaving out the mods known to run only in the game.
      {pack.noServerPack ? ` ${pack.noServerPack}` : ""}
    </p>
  );
}

function loaderName(loader: string): string {
  return loader === "neoforge" ? "NeoForge" : loader.charAt(0).toUpperCase() + loader.slice(1);
}
