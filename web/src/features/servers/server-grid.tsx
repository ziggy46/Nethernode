import { Search, Server as ServerIcon } from "lucide-react";
import { useId, useState } from "react";

import { EmptyState } from "@/components/page";
import { Select } from "@/components/ui/field";
import { type Server, ServerStatus } from "@/gen/nethernode/v1/servers_pb";
import { useNow } from "@/lib/use-now";

import { ServerCard } from "./server-card";

type Filter = "all" | "running" | "offline";

const isRunning = (s: Server) => s.status === ServerStatus.RUNNING || s.status === ServerStatus.STARTING;

/** Card grid, with search and a status filter unless `controls` is off (the overview, which links here). */
export function ServerGrid({ servers, controls = true }: { servers: Server[]; controls?: boolean }) {
  const [query, setQuery] = useState("");
  const [filter, setFilter] = useState<Filter>("all");
  const now = useNow();
  const ids = { search: useId(), filter: useId() };

  const visible = servers.filter(
    (s) =>
      s.name.toLowerCase().includes(query.trim().toLowerCase()) &&
      (filter === "all" || (filter === "running") === isRunning(s)),
  );

  return (
    <>
      <div className={controls ? "mb-[17px] flex items-center gap-2" : "hidden"}>
        <div className="relative min-w-[100px] flex-1">
          <Search
            className="pointer-events-none absolute top-1/2 left-3 size-[15px] -translate-y-1/2 text-text-muted"
            aria-hidden
          />
          <label htmlFor={ids.search} className="sr-only">
            Search servers
          </label>
          <input
            id={ids.search}
            type="search"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder="Search servers..."
            className="h-[37px] w-full rounded-md border border-border-strong bg-field pr-3 pl-[35px] text-label text-text-primary placeholder:text-text-muted focus-visible:border-primary focus-visible:outline-2 focus-visible:outline-primary focus-visible:outline-offset-2"
          />
        </div>
        <label htmlFor={ids.filter} className="sr-only">
          Filter by status
        </label>
        <Select
          id={ids.filter}
          value={filter}
          onChange={(e) => setFilter(e.target.value as Filter)}
          className="h-[37px] w-auto bg-field text-label text-text-secondary below-sm:max-w-[118px]"
        >
          <option value="all">All statuses</option>
          <option value="running">Running</option>
          <option value="offline">Not running</option>
        </Select>
      </div>
      {visible.length === 0 ? (
        <div className="rounded-lg border border-divider border-dashed bg-surface-subtle">
          <EmptyState compact icon={<ServerIcon />} title="No servers match">
            Try a different name or status.
          </EmptyState>
        </div>
      ) : (
        <div className="grid grid-cols-2 gap-4 below-xl:gap-3 below-sm:grid-cols-1">
          {visible.map((server) => (
            <ServerCard key={server.id} server={server} now={now} />
          ))}
        </div>
      )}
    </>
  );
}
