import { useQuery } from "@tanstack/react-query";
import { Download, ExternalLink, KeyRound, Plus, Search, TriangleAlert } from "lucide-react";
import { lazy, Suspense, useDeferredValue, useState } from "react";

import { EmptyState, PageHeader } from "@/components/page";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/field";
import { Platform, type Project, ProjectKind } from "@/gen/nethernode/v1/catalog_pb";
import { errorMessage } from "@/lib/api";
import { useCanManage } from "@/lib/session";
import { useDocumentTitle } from "@/lib/use-document-title";
import { cn } from "@/lib/utils";

import { PlatformTabs, ProjectIcon } from "../catalog/project-search";
import { compactCount, curseForgeStatusQuery, searchQuery } from "../catalog/queries";
import type { Draft } from "../servers/create/draft";
import { loaderName } from "../servers/format";

const CreateServerDialog = lazy(() =>
  import("@/features/servers/create-server-dialog").then((m) => ({ default: m.CreateServerDialog })),
);

/** Browse Modrinth and CurseForge modpacks and start a server from one. */
export function ModpacksPage() {
  useDocumentTitle("Modpacks");
  const canManage = useCanManage();
  const [platform, setPlatform] = useState(Platform.MODRINTH);
  const [text, setText] = useState("");
  const [page, setPage] = useState(0);
  const [preset, setPreset] = useState<Partial<Draft> | null>(null);
  const query = useDeferredValue(text.trim());
  const curseForgeKey = useQuery({ ...curseForgeStatusQuery, enabled: canManage });
  const needsKey = platform === Platform.CURSEFORGE && curseForgeKey.data === false;
  const results = useQuery({
    ...searchQuery({ platform, kind: ProjectKind.MODPACK, query, offset: page * 20 }),
    enabled: canManage && !needsKey,
  });
  const total = Number(results.data?.total ?? 0);

  const start = (p: Project) =>
    setPreset({
      software: "modpack",
      memoryMb: 6144,
      name: p.title.slice(0, 40),
      pack: {
        platform,
        project: p.id,
        title: p.title,
        iconUrl: p.iconUrl,
        versionId: "",
        versionName: "",
        mcVersion: "",
        loader: p.loaders[0] ?? "",
      },
    });

  return (
    <>
      <PageHeader
        eyebrow="Modpack library"
        title="Find your next adventure."
        description="Modpacks from Modrinth and CurseForge, ready to run as a server."
      />
      {!canManage ? (
        <EmptyState icon={<KeyRound />} title="Modpacks are for administrators">
          Ask an administrator to start a server from a modpack.
        </EmptyState>
      ) : (
        <>
          <div className="mb-5 flex flex-wrap items-center gap-3">
            <PlatformTabs
              value={platform}
              onChange={(p) => {
                setPlatform(p);
                setPage(0);
              }}
            />
            <div className="relative min-w-[220px] flex-1">
              <Search
                className="-translate-y-1/2 pointer-events-none absolute top-1/2 left-3 size-4 text-text-muted"
                aria-hidden
              />
              <Input
                type="search"
                aria-label="Search modpacks"
                placeholder="Search modpacks"
                className="pl-9"
                value={text}
                onChange={(e) => {
                  setText(e.target.value);
                  setPage(0);
                }}
              />
            </div>
          </div>
          {needsKey ? (
            <div className="rounded-lg border border-divider border-dashed bg-surface-subtle">
              <EmptyState
                icon={<KeyRound />}
                title="Browsing CurseForge needs an API key"
                action={
                  <Button
                    onClick={() =>
                      setPreset({
                        software: "modpack",
                        memoryMb: 6144,
                        pack: {
                          platform: Platform.CURSEFORGE,
                          project: "",
                          title: "",
                          iconUrl: "",
                          versionId: "",
                          versionName: "",
                          mcVersion: "",
                          loader: "",
                        },
                      })
                    }
                  >
                    <Plus />
                    Create from a pack address
                  </Button>
                }
              >
                Add a free key from console.curseforge.com in Settings. You can still create a server from a
                CurseForge pack's address.
              </EmptyState>
            </div>
          ) : results.isPending ? (
            <div className="grid grid-cols-3 gap-4 below-lg:grid-cols-2 below-sm:grid-cols-1" aria-busy>
              {["a", "b", "c", "d", "e", "f"].map((k) => (
                <div
                  key={k}
                  className="h-[230px] animate-pulse rounded-lg border border-divider bg-surface-subtle"
                />
              ))}
            </div>
          ) : results.isError ? (
            <EmptyState icon={<TriangleAlert />} title="Couldn't load modpacks">
              {errorMessage(results.error)}
            </EmptyState>
          ) : results.data.projects.length === 0 ? (
            <EmptyState icon={<Search />} title="No modpacks found">
              Try other words.
            </EmptyState>
          ) : (
            <>
              <ul
                className={cn(
                  "grid grid-cols-3 gap-4 below-lg:grid-cols-2 below-sm:grid-cols-1",
                  results.isPlaceholderData && "opacity-60",
                )}
              >
                {results.data.projects.map((p) => (
                  <li key={p.id}>
                    <article className="flex h-full flex-col rounded-lg border border-border bg-surface p-4">
                      <div className="mb-3 flex items-start gap-3">
                        <ProjectIcon url={p.iconUrl} size={56} />
                        <div className="min-w-0">
                          <h2 className="line-clamp-2 font-semibold text-body">{p.title}</h2>
                          {p.author && <p className="truncate text-meta text-text-muted">by {p.author}</p>}
                        </div>
                      </div>
                      <p className="mb-3 line-clamp-3 flex-1 text-label text-text-secondary">{p.summary}</p>
                      <div className="mb-3 flex flex-wrap gap-1.5 text-meta">
                        {p.loaders.map((l) => (
                          <span
                            key={l}
                            className="rounded-xs bg-surface-elevated px-[5px] py-px text-text-secondary"
                          >
                            {loaderName(l)}
                          </span>
                        ))}
                        {p.gameVersions.length > 0 && (
                          <span className="rounded-xs bg-surface-elevated px-[5px] py-px text-text-secondary">
                            {p.gameVersions[p.gameVersions.length - 1]}
                          </span>
                        )}
                        <span className="inline-flex items-center gap-1 text-text-muted">
                          <Download className="size-3" aria-hidden />
                          {compactCount(p.downloads)}
                        </span>
                      </div>
                      <div className="flex items-center gap-2">
                        <Button variant="primary" size="mini" onClick={() => start(p)}>
                          <Plus />
                          Create server
                        </Button>
                        <a
                          href={p.pageUrl}
                          target="_blank"
                          rel="noreferrer noopener"
                          className="ml-auto inline-flex items-center gap-1 text-meta text-text-secondary hover:text-text-primary"
                        >
                          Details
                          <ExternalLink className="size-3" aria-hidden />
                        </a>
                      </div>
                    </article>
                  </li>
                ))}
              </ul>
              {total > 20 && (
                <div className="mt-5 flex items-center justify-between text-meta text-text-muted">
                  <span>
                    {page * 20 + 1}–{Math.min(total, (page + 1) * 20)} of {compactCount(total)}
                  </span>
                  <span className="flex gap-1.5">
                    <Button size="mini" disabled={page === 0} onClick={() => setPage(page - 1)}>
                      Previous
                    </Button>
                    <Button size="mini" disabled={(page + 1) * 20 >= total} onClick={() => setPage(page + 1)}>
                      Next
                    </Button>
                  </span>
                </div>
              )}
            </>
          )}
        </>
      )}
      {preset && (
        <Suspense fallback={null}>
          <CreateServerDialog open onOpenChange={(o) => !o && setPreset(null)} preset={preset} />
        </Suspense>
      )}
    </>
  );
}
