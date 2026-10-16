import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  createColumnHelper,
  createSortedRowModel,
  rowSortingFeature,
  type SortingState,
  sortFn_alphanumeric,
  sortFn_basic,
  tableFeatures,
  useTable,
} from "@tanstack/react-table";
import {
  ArrowDown,
  ArrowUp,
  Gavel,
  ListChecks,
  LogOut,
  Search,
  TriangleAlert,
  UserCheck,
  Users,
  UserX,
} from "lucide-react";
import { useMemo, useState } from "react";
import { toast } from "sonner";

import { EmptyState } from "@/components/page";
import { Dot } from "@/components/status-badge";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { ErrorAlert, Input, Select } from "@/components/ui/field";
import type { PlayerRow } from "@/gen/nethernode/v1/players_pb";
import { errorMessage } from "@/lib/api";
import { useCanManage, useCanModerate } from "@/lib/session";
import { ago, duration } from "@/lib/time";
import { useNow } from "@/lib/use-now";
import { cn } from "@/lib/utils";

import { serversQuery } from "../servers/queries";
import { AllowlistDialog } from "./allowlist-dialog";
import { playerClient } from "./api";
import { BanDialog, type BanTarget } from "./ban-dialog";
import { PlayerBadges, PlayerDrawer } from "./player-drawer";
import { PlayerHead } from "./player-head";
import { playersKey, playersQuery } from "./queries";

const FILTERS = {
  all: ["All players", () => true],
  online: ["Online", (p) => p.online],
  operators: ["Operators", (p) => p.opLevel > 0],
  allowlisted: ["Allowlisted", (p) => p.allowlisted],
  banned: ["Banned", (p) => p.ban !== undefined],
} satisfies Record<string, [string, (p: PlayerRow) => boolean]>;
type Filter = keyof typeof FILTERS;

const VALID_NAME = /^[A-Za-z0-9_]{1,16}$/;

const features = tableFeatures({ rowSortingFeature, sortedRowModel: createSortedRowModel() });
const column = createColumnHelper<typeof features, PlayerRow>();

/** Players on one server, or (without `serverId`) everyone across all servers. */
export function PlayersView({ serverId, serverName }: { serverId?: string; serverName?: string }) {
  const client = useQueryClient();
  const now = useNow(30_000);
  const canModerate = useCanModerate();
  const canManage = useCanManage();
  const players = useQuery(playersQuery(serverId));
  const servers = useQuery({ ...serversQuery, enabled: !serverId });
  const [search, setSearch] = useState("");
  const [filter, setFilter] = useState<Filter>("all");
  const [sorting, setSorting] = useState<SortingState>([{ id: "lastSeen", desc: true }]);
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [open, setOpen] = useState<string | null>(null);
  const [banning, setBanning] = useState<BanTarget[] | null>(null);
  const [allowlistOpen, setAllowlistOpen] = useState(false);

  const caps = players.data?.capabilities;
  const rows = players.data?.players ?? [];
  const serverNames = useMemo(() => new Map(servers.data?.map((s) => [s.id, s.name])), [servers.data]);
  const term = search.trim().toLowerCase();
  const shown = useMemo(
    () =>
      rows.filter(
        (p) =>
          FILTERS[filter][1](p) && (!term || p.name.toLowerCase().includes(term) || p.uuid.startsWith(term)),
      ),
    [rows, filter, term],
  );
  const picked = rows.filter((p) => selected.has(p.uuid));
  const refresh = () => client.invalidateQueries({ queryKey: playersKey });
  const selectable = canModerate && (serverId ? caps?.backend !== "unavailable" : canManage);

  const columns = useMemo(
    () =>
      column.columns([
        column.display({
          id: "select",
          header: () => (
            <Checkbox
              aria-label="Select all shown players"
              checked={
                shown.length > 0 && shown.every((p) => selected.has(p.uuid))
                  ? true
                  : selected.size > 0
                    ? "indeterminate"
                    : false
              }
              onCheckedChange={(on) =>
                setSelected(on === true ? new Set(shown.map((p) => p.uuid)) : new Set())
              }
            />
          ),
          cell: ({ row }) => (
            <Checkbox
              aria-label={`Select ${row.original.name}`}
              checked={selected.has(row.original.uuid)}
              onClick={(e) => e.stopPropagation()}
              onCheckedChange={(on) =>
                setSelected((prev) => {
                  const next = new Set(prev);
                  if (on === true) next.add(row.original.uuid);
                  else next.delete(row.original.uuid);
                  return next;
                })
              }
            />
          ),
        }),
        column.accessor("name", {
          id: "name",
          header: "Player",
          sortFn: sortFn_alphanumeric,
          cell: ({ row }) => {
            const p = row.original;
            const where =
              !serverId && p.onlineServerIds.map((id) => serverNames.get(id) ?? "a server").join(", ");
            return (
              <span className="flex min-w-0 items-center gap-2.5">
                <PlayerHead uuid={p.uuid} name={p.name} size={28} />
                <span className="min-w-0">
                  <span className="flex items-center gap-1.5 font-medium">
                    <span className="truncate">{p.name}</span>
                    {p.online && <Dot className="text-success" />}
                    {p.online && <span className="sr-only">(online)</span>}
                  </span>
                  {where && <span className="block truncate text-meta text-text-muted">on {where}</span>}
                </span>
              </span>
            );
          },
        }),
        column.display({
          id: "status",
          header: "Status",
          cell: ({ row }) => {
            const p = row.original;
            return p.online || p.opLevel > 0 || p.allowlisted || p.ban ? (
              <PlayerBadges row={p} now={now} />
            ) : (
              <span className="text-text-muted">Offline</span>
            );
          },
        }),
        column.accessor((p) => (p.online ? Number.MAX_SAFE_INTEGER : Number(p.lastSeenMs)), {
          id: "lastSeen",
          header: "Last seen",
          sortFn: sortFn_basic,
          cell: ({ row }) =>
            row.original.online
              ? "Now"
              : Number(row.original.lastSeenMs)
                ? ago(row.original.lastSeenMs, now)
                : "—",
        }),
        column.accessor((p) => Number(p.playtimeMs), {
          id: "playtime",
          header: "Played",
          sortFn: sortFn_basic,
          cell: ({ getValue }) => (getValue() ? duration(getValue()) : "—"),
        }),
        column.accessor((p) => Number(p.firstSeenMs), {
          id: "firstSeen",
          header: "First seen",
          sortFn: sortFn_basic,
          cell: ({ getValue }) => (getValue() ? ago(getValue(), now) : "—"),
        }),
        column.accessor("lastIp", {
          id: "lastIp",
          header: "Last IP",
          enableSorting: false,
          cell: ({ getValue }) => <span className="font-mono">{getValue() || "—"}</span>,
        }),
      ]),
    [shown, selected, serverId, serverNames, now],
  );

  const table = useTable({
    features,
    columns: selectable ? columns : columns.slice(1),
    data: shown,
    state: { sorting },
    onSortingChange: setSorting,
    getRowId: (p) => p.uuid,
  });

  const bulk = useMutation({
    mutationFn: async ({ work }: { work: (p: PlayerRow) => Promise<unknown>; done: string }) => {
      const failed: string[] = [];
      for (const p of picked) {
        try {
          await work(p);
        } catch (err) {
          failed.push(`${p.name}: ${errorMessage(err)}`);
        }
      }
      if (failed.length) throw new Error(failed.join("\n"));
    },
    onSuccess: (_, { done }) => {
      toast.success(done);
      setSelected(new Set());
    },
    onSettled: refresh,
  });
  const runBulk = (done: string, work: (p: PlayerRow) => Promise<unknown>) => bulk.mutate({ work, done });
  const ref = (p: PlayerRow) => ({ name: p.name, uuid: p.uuid });

  if (players.isPending) {
    return (
      <div className="h-[320px] animate-pulse rounded-lg border border-divider bg-surface-subtle" aria-busy />
    );
  }
  if (players.isError) {
    return (
      <EmptyState icon={<TriangleAlert />} title="Couldn't load players">
        {errorMessage(players.error)}
      </EmptyState>
    );
  }

  const unknownName = VALID_NAME.test(search.trim()) && !rows.some((p) => p.name.toLowerCase() === term);

  return (
    <div>
      {serverId && caps?.backend === "unavailable" && (
        <p className="mb-4 rounded-md border border-warning/30 bg-warning-subtle px-3 py-2 text-label text-warning">
          {caps.unavailableReason}
        </p>
      )}
      {serverId && caps?.backend === "files" && (
        <p className="mb-4 rounded-md border border-divider bg-surface-subtle px-3 py-2 text-label text-text-secondary">
          {serverName} is stopped. Bans, operators, and the allowlist can still be changed; they apply when it
          starts.
        </p>
      )}
      <div className="mb-3.5 flex flex-wrap items-center gap-2">
        <div className="relative min-w-[200px] flex-1">
          <Search
            className="-translate-y-1/2 pointer-events-none absolute top-1/2 left-3 size-4 text-text-muted"
            aria-hidden
          />
          <Input
            type="search"
            aria-label="Search players"
            placeholder="Search by name or UUID"
            className="pl-9"
            value={search}
            onChange={(e) => setSearch(e.target.value)}
          />
        </div>
        <Select
          aria-label="Show"
          className="w-auto"
          value={filter}
          onChange={(e) => setFilter(e.target.value as Filter)}
        >
          {Object.entries(FILTERS).map(([value, [label]]) => (
            <option key={value} value={value}>
              {label}
            </option>
          ))}
        </Select>
        {serverId && serverName && caps?.allowlist && canModerate && (
          <Button onClick={() => setAllowlistOpen(true)}>
            <ListChecks />
            Allowlist{players.data.allowlistEnabled ? " (on)" : ""}
          </Button>
        )}
      </div>

      {picked.length > 0 && (
        <div
          className="mb-3 flex flex-wrap items-center gap-2 rounded-md border border-primary/40 bg-nav-active px-3 py-2"
          role="toolbar"
          aria-label="Selected players"
        >
          <span className="mr-2 text-label">{picked.length} selected</span>
          {serverId && caps?.kick && picked.some((p) => p.online) && (
            <Button
              size="mini"
              disabled={bulk.isPending}
              onClick={() =>
                runBulk("Kicked the selected players.", (p) =>
                  p.online
                    ? playerClient.kickPlayer({ serverId, player: ref(p), message: "" })
                    : Promise.resolve(),
                )
              }
            >
              <LogOut />
              Kick
            </Button>
          )}
          {serverId && caps?.allowlist && (
            <>
              <Button
                size="mini"
                disabled={bulk.isPending}
                onClick={() =>
                  runBulk("Added to the allowlist.", (p) =>
                    playerClient.setAllowlisted({ serverId, player: ref(p), allowed: true }),
                  )
                }
              >
                <UserCheck />
                Allowlist
              </Button>
              <Button
                size="mini"
                disabled={bulk.isPending}
                onClick={() =>
                  runBulk("Removed from the allowlist.", (p) =>
                    playerClient.setAllowlisted({ serverId, player: ref(p), allowed: false }),
                  )
                }
              >
                <UserX />
                Remove from allowlist
              </Button>
            </>
          )}
          {(serverId ? caps?.ban : canManage) && (
            <Button
              size="mini"
              variant="danger"
              disabled={bulk.isPending}
              onClick={() => setBanning(picked.map((p) => ({ name: p.name, uuid: p.uuid, ip: p.lastIp })))}
            >
              <Gavel />
              {serverId ? "Ban" : "Ban everywhere"}
            </Button>
          )}
          <Button size="mini" variant="ghost" className="ml-auto" onClick={() => setSelected(new Set())}>
            Clear
          </Button>
        </div>
      )}
      {bulk.isError && <ErrorAlert className="whitespace-pre-line">{errorMessage(bulk.error)}</ErrorAlert>}

      {shown.length === 0 ? (
        <div className="rounded-lg border border-divider border-dashed bg-surface-subtle">
          <EmptyState
            icon={<Users />}
            title={rows.length === 0 ? "No players yet" : "No players match"}
            action={
              unknownName &&
              canModerate &&
              (serverId ? caps?.ban : canManage) && (
                <Button variant="danger" onClick={() => setBanning([{ name: search.trim() }])}>
                  <Gavel />
                  Ban {search.trim()}
                </Button>
              )
            }
          >
            {rows.length === 0
              ? "Players appear here once they join through NetherNode, or when they're added to a list."
              : unknownName
                ? `${search.trim()} hasn't been seen yet. You can still ban them by name.`
                : "Try a different search or filter."}
          </EmptyState>
        </div>
      ) : (
        <div className="overflow-x-auto rounded-lg border border-border">
          <table className="w-full min-w-[640px] text-label">
            <thead className="bg-surface-subtle text-left text-meta text-text-muted">
              {table.getHeaderGroups().map((group) => (
                <tr key={group.id}>
                  {group.headers.map((header) => {
                    const sort = header.column.getIsSorted();
                    const canSort = header.column.getCanSort();
                    return (
                      <th
                        key={header.id}
                        scope="col"
                        aria-sort={sort === "asc" ? "ascending" : sort === "desc" ? "descending" : undefined}
                        className={cn(
                          "border-divider border-b px-3 py-2.5 font-medium",
                          header.column.id === "select" && "w-10",
                          header.column.id === "lastIp" && "below-md:hidden",
                          header.column.id === "firstSeen" && "below-md:hidden",
                        )}
                      >
                        {canSort ? (
                          <button
                            type="button"
                            className="inline-flex items-center gap-1 hover:text-text-primary"
                            onClick={header.column.getToggleSortingHandler()}
                          >
                            <table.FlexRender header={header} />
                            {sort === "asc" ? (
                              <ArrowUp className="size-3" aria-hidden />
                            ) : sort === "desc" ? (
                              <ArrowDown className="size-3" aria-hidden />
                            ) : null}
                          </button>
                        ) : (
                          <table.FlexRender header={header} />
                        )}
                      </th>
                    );
                  })}
                </tr>
              ))}
            </thead>
            <tbody>
              {table.getRowModel().rows.map((row) => (
                <tr
                  key={row.id}
                  tabIndex={0}
                  aria-label={`${row.original.name}: open details`}
                  onClick={() => setOpen(row.original.uuid)}
                  onKeyDown={(e) => {
                    if (e.target === e.currentTarget && (e.key === "Enter" || e.key === " ")) {
                      e.preventDefault();
                      setOpen(row.original.uuid);
                    }
                  }}
                  className={cn(
                    "cursor-pointer border-divider border-b last:border-b-0 hover:bg-surface-subtle focus-visible:outline-2 focus-visible:outline-primary focus-visible:-outline-offset-2",
                    selected.has(row.id) && "bg-nav-active",
                  )}
                >
                  {row.getAllCells().map((cell) => (
                    <td
                      key={cell.id}
                      className={cn(
                        "px-3 py-2 align-middle",
                        cell.column.id !== "name" &&
                          cell.column.id !== "status" &&
                          "whitespace-nowrap text-text-secondary",
                        (cell.column.id === "lastIp" || cell.column.id === "firstSeen") && "below-md:hidden",
                      )}
                    >
                      <table.FlexRender cell={cell} />
                    </td>
                  ))}
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}

      <PlayerDrawer uuid={open} serverId={serverId} serverName={serverName} onClose={() => setOpen(null)} />
      {banning && (
        <BanDialog
          open
          onOpenChange={(o) => !o && setBanning(null)}
          players={banning}
          serverId={serverId}
          serverName={serverName}
          canBanEverywhere={canManage}
          onDone={() => {
            setSelected(new Set());
            void refresh();
          }}
        />
      )}
      {serverId && serverName && (
        <AllowlistDialog
          open={allowlistOpen}
          onOpenChange={setAllowlistOpen}
          serverId={serverId}
          serverName={serverName}
          enabled={players.data.allowlistEnabled}
          players={rows}
          onChanged={refresh}
        />
      )}
    </div>
  );
}
