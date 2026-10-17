import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  Backpack,
  Copy,
  Gavel,
  LogOut,
  Navigation,
  PackagePlus,
  ShieldCheck,
  ShieldOff,
  UserCheck,
  UserX,
} from "lucide-react";
import { Tabs } from "radix-ui";
import { useId, useState } from "react";
import { toast } from "sonner";

import { EmptyState } from "@/components/page";
import { Badge, Dot } from "@/components/status-badge";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogTitle } from "@/components/ui/dialog";
import { ErrorAlert, Input, Select } from "@/components/ui/field";
import { type Capabilities, GameMode, type ItemStack, type PlayerRow } from "@/gen/nethernode/v1/players_pb";
import { errorMessage } from "@/lib/api";
import { useCanManage, useCanModerate } from "@/lib/session";
import { ago, dateTime, duration, until } from "@/lib/time";
import { useNow } from "@/lib/use-now";
import { cn } from "@/lib/utils";

import { playerClient } from "./api";
import { BanDialog } from "./ban-dialog";
import { ItemIconsContext, itemIconsQuery, Slot, statName } from "./items";
import { PlayerHead } from "./player-head";
import { playerDataQuery, playerQuery, playersKey } from "./queries";

const GAME_MODES: [GameMode, string][] = [
  [GameMode.SURVIVAL, "Survival"],
  [GameMode.CREATIVE, "Creative"],
  [GameMode.ADVENTURE, "Adventure"],
  [GameMode.SPECTATOR, "Spectator"],
];

export function PlayerBadges({ row, now }: { row: PlayerRow; now: number }) {
  return (
    <span className="flex flex-wrap gap-1.5">
      {row.online && (
        <Badge tone="success" icon={<Dot />}>
          Online
        </Badge>
      )}
      {row.opLevel > 0 && <Badge tone="warning">OP {row.opLevel}</Badge>}
      {row.allowlisted && <Badge tone="info">Allowlisted</Badge>}
      {row.ban && (
        <Badge tone="danger">
          Banned
          {row.ban.global ? " everywhere" : ""}
          {row.ban.expiresMs !== undefined ? ` · ends ${until(row.ban.expiresMs, now)}` : ""}
        </Badge>
      )}
    </span>
  );
}

/** Details and actions for one player, as a side sheet. */
export function PlayerDrawer({
  uuid,
  serverId,
  serverName,
  onClose,
}: {
  uuid: string | null;
  serverId?: string;
  serverName?: string;
  onClose: () => void;
}) {
  return (
    <Dialog open={uuid !== null} onOpenChange={(open) => !open && onClose()}>
      <DialogContent className="top-0 right-0 left-auto h-dvh max-h-dvh w-[min(560px,100vw)] max-w-[100vw] translate-x-0 translate-y-0 rounded-none border-y-0 border-r-0 p-0">
        {uuid && <DrawerBody uuid={uuid} serverId={serverId} serverName={serverName} />}
      </DialogContent>
    </Dialog>
  );
}

function DrawerBody({
  uuid,
  serverId,
  serverName,
}: {
  uuid: string;
  serverId?: string;
  serverName?: string;
}) {
  const now = useNow();
  const detail = useQuery(playerQuery(uuid, serverId));
  const player = detail.data?.player;
  const caps = detail.data?.capabilities;

  if (detail.isPending) {
    return (
      <>
        <DialogTitle className="sr-only">Loading player</DialogTitle>
        <div className="m-6 h-40 animate-pulse rounded-md bg-surface-subtle" aria-busy />
      </>
    );
  }
  if (detail.isError || !player) {
    return (
      <div className="p-6">
        <DialogTitle>Couldn't load this player</DialogTitle>
        <DialogDescription className="mt-2">{errorMessage(detail.error)}</DialogDescription>
      </div>
    );
  }

  return (
    <div className="flex h-full flex-col">
      <header className="flex items-start gap-4 border-divider border-b p-6 pr-16">
        <PlayerHead uuid={player.uuid} name={player.name} size={64} />
        <div className="min-w-0">
          <DialogTitle className="truncate">{player.name}</DialogTitle>
          <DialogDescription asChild>
            <button
              type="button"
              className="mt-1 flex items-center gap-1.5 font-mono text-meta text-text-muted hover:text-text-primary"
              onClick={() =>
                navigator.clipboard.writeText(player.uuid).then(() => toast.success("UUID copied."))
              }
            >
              {player.uuid}
              <Copy className="size-3" aria-hidden />
              <span className="sr-only">Copy UUID</span>
            </button>
          </DialogDescription>
          <div className="mt-2.5">
            <PlayerBadges row={player} now={now} />
          </div>
        </div>
      </header>
      <div className="flex-1 overflow-y-auto">
        <Actions player={player} caps={caps} serverId={serverId} serverName={serverName} />
        <Tabs.Root defaultValue="overview" className="px-6 pb-6">
          <Tabs.List aria-label="Player details" className="mb-4 flex gap-5 border-divider border-b">
            {[
              ["overview", "Overview"],
              ...(serverId
                ? [
                    ["inventory", "Inventory"],
                    ["stats", "Stats"],
                  ]
                : []),
              ["sessions", "Sessions"],
              ["notes", "Notes"],
            ].map(([value, label]) => (
              <Tabs.Trigger
                key={value}
                value={value ?? ""}
                className="-mb-px border-transparent border-b-2 pt-2 pb-2.5 text-label text-text-muted hover:text-text-primary data-[state=active]:border-primary data-[state=active]:text-primary"
              >
                {label}
              </Tabs.Trigger>
            ))}
          </Tabs.List>
          <Tabs.Content value="overview">
            <dl className="text-meta">
              <Fact label="First seen">
                {Number(player.firstSeenMs)
                  ? dateTime(player.firstSeenMs)
                  : "Never joined through NetherNode"}
              </Fact>
              <Fact label="Last seen">
                {player.online ? "Now" : Number(player.lastSeenMs) ? ago(player.lastSeenMs, now) : "—"}
              </Fact>
              <Fact label="Time played">{Number(player.playtimeMs) ? duration(player.playtimeMs) : "—"}</Fact>
              <Fact label="Last IP address">
                <span className="font-mono">{player.lastIp || "—"}</span>
              </Fact>
              {player.ban && (
                <Fact label="Ban">
                  {player.ban.reason || "No reason given"}
                  {player.ban.expiresMs !== undefined
                    ? ` · until ${dateTime(player.ban.expiresMs)}`
                    : " · permanent"}
                </Fact>
              )}
            </dl>
          </Tabs.Content>
          {serverId && (
            <>
              <Tabs.Content value="inventory">
                <Inventory serverId={serverId} uuid={uuid} online={player.online} />
              </Tabs.Content>
              <Tabs.Content value="stats">
                <Stats serverId={serverId} uuid={uuid} />
              </Tabs.Content>
            </>
          )}
          <Tabs.Content value="sessions">
            {(detail.data?.sessions.length ?? 0) === 0 ? (
              <p className="text-label text-text-muted">No sessions recorded yet.</p>
            ) : (
              <ol className="relative ml-2 border-divider border-l">
                {detail.data?.sessions.map((s) => (
                  <li key={`${s.serverId}-${s.joinedMs}`} className="mb-4 ml-4">
                    <span
                      aria-hidden
                      className={cn(
                        "absolute -left-[5px] mt-1.5 size-2.5 rounded-full border-2 border-surface",
                        s.leftMs === undefined ? "bg-success" : "bg-border-strong",
                      )}
                    />
                    <p className="text-label">
                      {dateTime(s.joinedMs)}
                      {!serverId && <span className="text-text-muted"> · {s.serverName}</span>}
                    </p>
                    <p className="text-meta text-text-muted">
                      {s.leftMs === undefined
                        ? "Online now"
                        : `Played ${duration(Number(s.leftMs) - Number(s.joinedMs))}`}
                      {s.ip && <span className="font-mono"> · {s.ip}</span>}
                    </p>
                  </li>
                ))}
              </ol>
            )}
          </Tabs.Content>
          <Tabs.Content value="notes">
            <Notes uuid={uuid} initial={detail.data?.notes ?? ""} known={Number(player.firstSeenMs) > 0} />
          </Tabs.Content>
        </Tabs.Root>
      </div>
    </div>
  );
}

function Fact({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div className="flex justify-between gap-4 border-divider border-b py-[11px] last:border-b-0">
      <dt className="text-text-muted">{label}</dt>
      <dd className="text-right">{children}</dd>
    </div>
  );
}

function Actions({
  player,
  caps,
  serverId,
  serverName,
}: {
  player: PlayerRow;
  caps?: Capabilities;
  serverId?: string;
  serverName?: string;
}) {
  const client = useQueryClient();
  const canModerate = useCanModerate();
  const canManage = useCanManage();
  const ids = { level: useId(), mode: useId(), tpPlayer: useId(), item: useId(), count: useId() };
  const [banOpen, setBanOpen] = useState(false);
  const [level, setLevel] = useState(String(player.opLevel || 4));
  const [mode, setMode] = useState(String(GameMode.SURVIVAL));
  const [tp, setTp] = useState({ player: "", x: "", y: "", z: "" });
  const [give, setGive] = useState({ item: "", count: "1" });
  const ref = { name: player.name, uuid: player.uuid };
  const refresh = () => client.invalidateQueries({ queryKey: playersKey });
  const act = useMutation({
    mutationFn: async ({ work }: { work: () => Promise<unknown>; done: string }) => work(),
    onSuccess: (reply, { done }) => {
      toast.success(done);
      // Some actions only partly apply (a server that can't take the change
      // until it restarts); say so.
      const warnings = (reply as { warnings?: string[] } | undefined)?.warnings ?? [];
      for (const w of warnings) toast.warning(w, { duration: 10_000 });
      void refresh();
    },
  });
  const run = (done: string, work: () => Promise<unknown>) => act.mutate({ work, done });

  if (!canModerate) return null;
  if (!serverId) {
    // All-servers view: bans that cover every server.
    if (!canManage) return null;
    return (
      <section className="flex flex-wrap gap-2 border-divider border-b px-6 py-4" aria-label="Actions">
        {act.isError && <ErrorAlert className="w-full">{errorMessage(act.error)}</ErrorAlert>}
        {player.ban?.global ? (
          <Button
            size="mini"
            disabled={act.isPending}
            onClick={() =>
              run(`Unbanned ${player.name} everywhere.`, () => playerClient.unbanPlayer({ player: ref }))
            }
          >
            <ShieldCheck />
            Unban everywhere
          </Button>
        ) : (
          <Button size="mini" variant="danger" onClick={() => setBanOpen(true)}>
            <Gavel />
            Ban everywhere
          </Button>
        )}
        <BanDialog
          open={banOpen}
          onOpenChange={setBanOpen}
          players={[{ name: player.name, uuid: player.uuid, ip: player.lastIp }]}
          canBanEverywhere
          onDone={refresh}
        />
      </section>
    );
  }

  const unavailable = caps?.backend === "unavailable";
  return (
    <section className="border-divider border-b px-6 py-4" aria-label="Actions">
      {unavailable && (
        <p className="mb-3 rounded-md border border-warning/30 bg-warning-subtle px-3 py-2 text-meta text-warning">
          {caps?.unavailableReason}
        </p>
      )}
      {caps?.backend === "files" && (
        <p className="mb-3 text-meta text-text-muted">
          {serverName} is stopped: changes are saved to its files and apply when it starts.
        </p>
      )}
      {act.isError && <ErrorAlert>{errorMessage(act.error)}</ErrorAlert>}
      <div className="mb-3 flex flex-wrap gap-2">
        {caps?.kick && player.online && (
          <Button
            size="mini"
            disabled={act.isPending}
            onClick={() =>
              run(`Kicked ${player.name}.`, () =>
                playerClient.kickPlayer({ serverId, player: ref, message: "" }),
              )
            }
          >
            <LogOut />
            Kick
          </Button>
        )}
        {caps?.ban &&
          (player.ban ? (
            <Button
              size="mini"
              disabled={act.isPending}
              onClick={() =>
                run(`Unbanned ${player.name}.`, () => playerClient.unbanPlayer({ serverId, player: ref }))
              }
            >
              <ShieldCheck />
              Unban
            </Button>
          ) : (
            <Button size="mini" variant="danger" onClick={() => setBanOpen(true)}>
              <Gavel />
              Ban
            </Button>
          ))}
        {caps?.allowlist && (
          <Button
            size="mini"
            disabled={act.isPending}
            onClick={() =>
              run(
                player.allowlisted
                  ? `Removed ${player.name} from the allowlist.`
                  : `Added ${player.name} to the allowlist.`,
                () => playerClient.setAllowlisted({ serverId, player: ref, allowed: !player.allowlisted }),
              )
            }
          >
            {player.allowlisted ? <UserX /> : <UserCheck />}
            {player.allowlisted ? "Remove from allowlist" : "Add to allowlist"}
          </Button>
        )}
      </div>
      {caps?.op && (
        <div className="mb-3 flex flex-wrap items-center gap-2">
          {caps.opLevels && (
            <Select
              id={ids.level}
              aria-label="Operator level"
              className="h-[27px] w-auto py-0 text-meta"
              value={level}
              onChange={(e) => setLevel(e.target.value)}
            >
              {[1, 2, 3, 4].map((l) => (
                <option key={l} value={l}>
                  Level {l}
                </option>
              ))}
            </Select>
          )}
          <Button
            size="mini"
            disabled={act.isPending}
            onClick={() =>
              run(`${player.name} is now an operator.`, () =>
                playerClient.setOperator({ serverId, player: ref, level: Number(level) }),
              )
            }
          >
            <ShieldCheck />
            {player.opLevel > 0 ? "Change operator level" : "Make operator"}
          </Button>
          {player.opLevel > 0 && (
            <Button
              size="mini"
              variant="ghost"
              disabled={act.isPending}
              onClick={() =>
                run(`${player.name} is no longer an operator.`, () =>
                  playerClient.setOperator({ serverId, player: ref, level: 0 }),
                )
              }
            >
              <ShieldOff />
              Remove operator
            </Button>
          )}
        </div>
      )}
      {caps?.gameMode && player.online && (
        <details className="mb-2 rounded-md border border-divider px-3 py-2">
          <summary className="cursor-pointer text-label text-text-secondary">
            Game mode, teleport, give
          </summary>
          <div className="mt-3 flex flex-col gap-3">
            <div className="flex items-center gap-2">
              <Select
                id={ids.mode}
                aria-label="Game mode"
                className="h-[27px] w-auto py-0 text-meta"
                value={mode}
                onChange={(e) => setMode(e.target.value)}
              >
                {GAME_MODES.map(([value, label]) => (
                  <option key={value} value={value}>
                    {label}
                  </option>
                ))}
              </Select>
              <Button
                size="mini"
                disabled={act.isPending}
                onClick={() =>
                  run("Game mode changed.", () =>
                    playerClient.setGameMode({ serverId, player: ref, mode: Number(mode) }),
                  )
                }
              >
                Set game mode
              </Button>
            </div>
            <form
              className="flex flex-wrap items-center gap-2"
              onSubmit={(e) => {
                e.preventDefault();
                const toCoords = !tp.player.trim();
                run("Teleported.", () =>
                  playerClient.teleportPlayer({
                    serverId,
                    player: ref,
                    destination: toCoords
                      ? {
                          case: "toCoordinates",
                          value: { x: Number(tp.x), y: Number(tp.y), z: Number(tp.z) },
                        }
                      : { case: "toPlayer", value: tp.player.trim() },
                  }),
                );
              }}
            >
              <Input
                aria-label="Teleport to player"
                placeholder="To player"
                className="h-[27px] w-28 text-meta"
                value={tp.player}
                onChange={(e) => setTp({ ...tp, player: e.target.value })}
              />
              <span className="text-meta text-text-muted">or</span>
              {(["x", "y", "z"] as const).map((axis) => (
                <Input
                  key={axis}
                  aria-label={axis.toUpperCase()}
                  placeholder={axis.toUpperCase()}
                  inputMode="decimal"
                  className="h-[27px] w-16 text-meta"
                  value={tp[axis]}
                  onChange={(e) => setTp({ ...tp, [axis]: e.target.value })}
                />
              ))}
              <Button
                type="submit"
                size="mini"
                disabled={
                  act.isPending ||
                  (!tp.player.trim() &&
                    [tp.x, tp.y, tp.z].some((v) => v.trim() === "" || Number.isNaN(Number(v))))
                }
              >
                <Navigation />
                Teleport
              </Button>
            </form>
            <form
              className="flex flex-wrap items-center gap-2"
              onSubmit={(e) => {
                e.preventDefault();
                run(`Gave ${give.count} ${give.item}.`, () =>
                  playerClient.giveItem({
                    serverId,
                    player: ref,
                    item: give.item.trim(),
                    count: Number(give.count),
                  }),
                );
              }}
            >
              <Input
                aria-label="Item ID"
                placeholder="minecraft:diamond"
                className="h-[27px] w-44 font-mono text-meta"
                value={give.item}
                onChange={(e) => setGive({ ...give, item: e.target.value })}
              />
              <Input
                aria-label="Count"
                inputMode="numeric"
                className="h-[27px] w-16 text-meta"
                value={give.count}
                onChange={(e) => setGive({ ...give, count: e.target.value })}
              />
              <Button
                type="submit"
                size="mini"
                disabled={act.isPending || !give.item.trim() || !(Number(give.count) >= 1)}
              >
                <PackagePlus />
                Give
              </Button>
            </form>
          </div>
        </details>
      )}
      <BanDialog
        open={banOpen}
        onOpenChange={setBanOpen}
        players={[{ name: player.name, uuid: player.uuid, ip: player.lastIp }]}
        serverId={serverId}
        serverName={serverName}
        canBanEverywhere={canManage}
        onDone={refresh}
      />
    </section>
  );
}

function Inventory({ serverId, uuid, online }: { serverId: string; uuid: string; online: boolean }) {
  // Online players are read from the running server: keep it current.
  const data = useQuery({ ...playerDataQuery(serverId, uuid), refetchInterval: online ? 2000 : false });
  const d0 = data.data;
  const ids = d0
    ? [
        ...new Set(
          [...d0.inventory, ...d0.enderChest, d0.head, d0.chest, d0.legs, d0.feet, d0.offhand]
            .filter((i): i is ItemStack => !!i)
            .map((i) => i.id),
        ),
      ]
    : [];
  const icons = useQuery(itemIconsQuery(serverId, ids)).data;
  if (data.isPending) return <div className="h-40 animate-pulse rounded-md bg-surface-subtle" aria-busy />;
  if (data.isError) return <ErrorAlert>{errorMessage(data.error)}</ErrorAlert>;
  const d = data.data;
  if (!d.found) {
    return (
      <EmptyState compact icon={<Backpack />} title="No saved data yet">
        The server saves a player's inventory when they leave and during autosaves.
      </EmptyState>
    );
  }
  const bySlot = (items: ItemStack[]) => new Map(items.map((i) => [Number(i.slot), i]));
  const inv = bySlot(d.inventory);
  const ender = bySlot(d.enderChest);
  const grid = (slots: number[], map: Map<number, ItemStack>, selected?: number) => (
    <div className="grid grid-cols-9 gap-1">
      {slots.map((s) => (
        <Slot key={s} item={map.get(s)} selected={s === selected} />
      ))}
    </div>
  );
  const range = (from: number, to: number) => Array.from({ length: to - from }, (_, i) => from + i);
  return (
    <ItemIconsContext.Provider value={icons}>
      <div className="flex flex-col gap-4">
        {d.live ? (
          <p className="flex items-center gap-1.5 text-meta text-text-muted">
            <span aria-hidden className="inline-block size-1.5 rounded-full bg-success" />
            Live: read from the running server every couple of seconds.
          </p>
        ) : (
          <p className="text-meta text-text-muted">
            As of the server's last save ({dateTime(d.savedMs)})
            {online ? "; live details appear once the server answers" : ""}.
          </p>
        )}
        <dl className="grid grid-cols-2 gap-x-4 text-meta">
          <Fact label="Health">{(d.health / 2).toFixed(1)} ♥</Fact>
          <Fact label="Food">{String(d.food)} / 20</Fact>
          <Fact label="Level">{String(d.xpLevel)}</Fact>
          <Fact label="Game mode">{GAME_MODES.find(([v]) => v === Number(d.gameMode) + 1)?.[1] ?? "—"}</Fact>
          <Fact label="Dimension">{d.dimension.replace("minecraft:", "").replaceAll("_", " ")}</Fact>
          <Fact label="Position">
            <span className="font-mono">{d.position.map((p) => Math.round(p)).join(", ")}</span>
          </Fact>
        </dl>
        <div className="flex flex-col gap-2">
          <div className="grid grid-cols-9 gap-1">
            <Slot item={d.head} label="Head" />
            <Slot item={d.chest} label="Body" />
            <Slot item={d.legs} label="Legs" />
            <Slot item={d.feet} label="Feet" />
            <span className="col-start-9">
              <Slot item={d.offhand} label="Off" />
            </span>
          </div>
          {grid(range(9, 36), inv)}
          {grid(range(0, 9), inv, Number(d.selectedSlot))}
        </div>
        <div>
          <h3 className="mb-2 text-label text-text-secondary">Ender chest</h3>
          {grid(range(0, 27), ender)}
        </div>
      </div>
    </ItemIconsContext.Provider>
  );
}

const HIGHLIGHTS: [string, string, (v: number) => string][] = [
  ["minecraft:play_time", "Time played", (v) => duration(v * 50)],
  ["minecraft:deaths", "Deaths", String],
  ["minecraft:mob_kills", "Mobs killed", String],
  ["minecraft:player_kills", "Players killed", String],
  ["minecraft:walk_one_cm", "Distance walked", (v) => `${(v / 100_000).toFixed(1)} km`],
  ["minecraft:jump", "Jumps", String],
];

function Stats({ serverId, uuid }: { serverId: string; uuid: string }) {
  const data = useQuery(playerDataQuery(serverId, uuid));
  const [category, setCategory] = useState("minecraft:custom");
  if (data.isPending) return <div className="h-40 animate-pulse rounded-md bg-surface-subtle" aria-busy />;
  if (data.isError) return <ErrorAlert>{errorMessage(data.error)}</ErrorAlert>;
  const stats = data.data.stats;
  if (stats.length === 0) return <p className="text-label text-text-muted">No statistics saved yet.</p>;
  const custom = new Map(
    stats.filter((s) => s.category === "minecraft:custom").map((s) => [s.key, Number(s.value)]),
  );
  const categories = [...new Set(stats.map((s) => s.category))];
  const shown = stats.filter((s) => s.category === category).slice(0, 50);
  return (
    <div>
      <div className="mb-4 grid grid-cols-3 gap-2 below-sm:grid-cols-2">
        {HIGHLIGHTS.map(([key, label, format]) => (
          <div key={key} className="rounded-md border border-divider p-2.5">
            <p className="text-meta text-text-muted">{label}</p>
            <p className="font-semibold text-label tabular-nums">{format(custom.get(key) ?? 0)}</p>
          </div>
        ))}
      </div>
      <Select
        aria-label="Statistics category"
        className="mb-2 h-9"
        value={category}
        onChange={(e) => setCategory(e.target.value)}
      >
        {categories.map((c) => (
          <option key={c} value={c}>
            {statName(c)}
          </option>
        ))}
      </Select>
      <table className="w-full text-meta">
        <tbody>
          {shown.map((s) => (
            <tr key={s.key} className="border-divider border-b last:border-b-0">
              <td className="py-1.5 text-text-secondary">{statName(s.key)}</td>
              <td className="py-1.5 text-right tabular-nums">{String(s.value)}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

function Notes({ uuid, initial, known }: { uuid: string; initial: string; known: boolean }) {
  const client = useQueryClient();
  const canModerate = useCanModerate();
  const [text, setText] = useState(initial);
  const save = useMutation({
    mutationFn: () => playerClient.setPlayerNotes({ uuid, notes: text }),
    onSuccess: () => {
      toast.success("Notes saved.");
      void client.invalidateQueries({ queryKey: playersKey });
    },
  });
  if (!known) {
    return (
      <p className="text-label text-text-muted">
        Notes can be added once this player has joined through NetherNode.
      </p>
    );
  }
  return (
    <div>
      {save.isError && <ErrorAlert>{errorMessage(save.error)}</ErrorAlert>}
      <textarea
        aria-label="Notes about this player"
        rows={6}
        maxLength={4000}
        disabled={!canModerate}
        value={text}
        onChange={(e) => setText(e.target.value)}
        placeholder="Only visible to panel users."
        className="block w-full rounded-md border border-border-strong bg-sidebar px-3 py-2 text-label text-text-primary placeholder:text-text-muted focus-visible:border-primary focus-visible:outline-2 focus-visible:outline-primary focus-visible:outline-offset-2"
      />
      {canModerate && (
        <Button
          className="mt-2"
          size="mini"
          variant="primary"
          disabled={save.isPending || text === initial}
          onClick={() => save.mutate()}
        >
          {save.isPending ? "Saving…" : "Save notes"}
        </Button>
      )}
    </div>
  );
}
