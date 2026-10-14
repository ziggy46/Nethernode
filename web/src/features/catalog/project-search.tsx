import { useQuery } from "@tanstack/react-query";
import { Box, Download, Search, TriangleAlert } from "lucide-react";
import { useDeferredValue, useState } from "react";

import { EmptyState } from "@/components/page";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/field";
import { Platform, type Project, ProjectKind } from "@/gen/nethernode/v1/catalog_pb";
import { errorMessage } from "@/lib/api";
import { cn } from "@/lib/utils";

import { compactCount, searchQuery } from "./queries";

/** A project's icon (served through NetherNode), or a plain box. */
export function ProjectIcon({
  url,
  size = 40,
  className,
}: {
  url: string;
  size?: number;
  className?: string;
}) {
  const [failed, setFailed] = useState(false);
  if (!url || failed) {
    return (
      <span
        aria-hidden
        className={cn(
          "grid shrink-0 place-items-center rounded-md bg-surface-subtle text-text-muted",
          className,
        )}
        style={{ width: size, height: size }}
      >
        <Box className="size-1/2" />
      </span>
    );
  }
  return (
    <img
      src={url}
      alt=""
      aria-hidden
      width={size}
      height={size}
      loading="lazy"
      onError={() => setFailed(true)}
      className={cn("shrink-0 rounded-md object-cover", className)}
      style={{ width: size, height: size }}
    />
  );
}

export function PlatformTabs({
  value,
  onChange,
}: {
  value: Platform;
  onChange: (platform: Platform) => void;
}) {
  return (
    <div role="tablist" aria-label="Where from" className="inline-flex rounded-md border border-border p-0.5">
      {[Platform.MODRINTH, Platform.CURSEFORGE].map((p) => (
        <button
          key={p}
          type="button"
          role="tab"
          aria-selected={value === p}
          onClick={() => onChange(p)}
          className={cn(
            "rounded-sm px-3 py-1.5 text-label",
            value === p
              ? "bg-nav-active text-nav-active-foreground"
              : "text-text-secondary hover:text-text-primary",
          )}
        >
          {p === Platform.MODRINTH ? "Modrinth" : "CurseForge"}
        </button>
      ))}
    </div>
  );
}

/**
 * Searches modpacks or mods on one platform. `mcVersion` and `loader`
 * narrow mod searches to what fits a server.
 */
export function ProjectSearch({
  platform,
  kind,
  mcVersion,
  loader,
  selectedId,
  onSelect,
  action,
}: {
  platform: Platform;
  kind: ProjectKind;
  mcVersion?: string;
  loader?: string;
  selectedId?: string;
  onSelect?: (project: Project) => void;
  /** A button per result instead of selecting rows. */
  action?: (project: Project) => React.ReactNode;
}) {
  const [text, setText] = useState("");
  const [page, setPage] = useState(0);
  const query = useDeferredValue(text.trim());
  const results = useQuery(searchQuery({ platform, kind, query, mcVersion, loader, offset: page * 20 }));
  const what = kind === ProjectKind.MODPACK ? "modpacks" : "mods";
  const total = Number(results.data?.total ?? 0);

  return (
    <div>
      <div className="relative mb-3">
        <Search
          className="-translate-y-1/2 pointer-events-none absolute top-1/2 left-3 size-4 text-text-muted"
          aria-hidden
        />
        <Input
          type="search"
          aria-label={`Search ${what}`}
          placeholder={`Search ${what}`}
          className="pl-9"
          value={text}
          onChange={(e) => {
            setText(e.target.value);
            setPage(0);
          }}
        />
      </div>
      {results.isPending ? (
        <div className="flex flex-col gap-2" aria-busy>
          {[0, 1, 2].map((i) => (
            <div key={i} className="h-[62px] animate-pulse rounded-md bg-surface-subtle" />
          ))}
        </div>
      ) : results.isError ? (
        <EmptyState compact icon={<TriangleAlert />} title={`Couldn't search ${what}`}>
          {errorMessage(results.error)}
        </EmptyState>
      ) : results.data.projects.length === 0 ? (
        <EmptyState compact icon={<Search />} title={`No ${what} found`}>
          {mcVersion
            ? `Nothing matches for Minecraft ${mcVersion}${loader ? ` with ${loader}` : ""}.`
            : "Try other words."}
        </EmptyState>
      ) : (
        <ul
          className={cn("flex flex-col gap-1.5", results.isPlaceholderData && "opacity-60")}
          aria-busy={results.isFetching}
        >
          {results.data.projects.map((p) => {
            const selected = p.id === selectedId;
            const body = (
              <>
                <ProjectIcon url={p.iconUrl} />
                <span className="min-w-0 flex-1">
                  <span className="flex items-baseline gap-2">
                    <span className="truncate font-medium text-label">{p.title}</span>
                    {p.author && (
                      <span className="shrink-0 truncate text-meta text-text-muted">by {p.author}</span>
                    )}
                  </span>
                  <span className="line-clamp-1 text-meta text-text-secondary">{p.summary}</span>
                  <span className="mt-0.5 flex flex-wrap items-center gap-x-2 text-meta text-text-muted">
                    <span className="inline-flex items-center gap-1">
                      <Download className="size-3" aria-hidden />
                      {compactCount(p.downloads)}
                    </span>
                    {p.loaders.length > 0 && <span>{p.loaders.join(", ")}</span>}
                    {p.gameVersions.length > 0 && (
                      <span>
                        {p.gameVersions.length > 1
                          ? `${p.gameVersions[0]}–${p.gameVersions[p.gameVersions.length - 1]}`
                          : p.gameVersions[0]}
                      </span>
                    )}
                  </span>
                </span>
              </>
            );
            return (
              <li key={p.id}>
                {action ? (
                  <div className="flex items-center gap-3 rounded-md border border-divider px-3 py-2.5">
                    {body}
                    {action(p)}
                  </div>
                ) : (
                  <button
                    type="button"
                    aria-pressed={selected}
                    onClick={() => onSelect?.(p)}
                    className={cn(
                      "flex w-full items-center gap-3 rounded-md border px-3 py-2.5 text-left",
                      selected ? "border-primary bg-nav-active" : "border-divider hover:border-border-strong",
                    )}
                  >
                    {body}
                  </button>
                )}
              </li>
            );
          })}
        </ul>
      )}
      {total > 20 && (
        <div className="mt-3 flex items-center justify-between text-meta text-text-muted">
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
    </div>
  );
}
