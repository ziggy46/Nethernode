import { keepPreviousData, useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  CalendarClock,
  Ellipsis,
  History,
  Megaphone,
  Pencil,
  Play,
  Plus,
  RotateCcw,
  Square,
  SquareTerminal,
  Trash2,
} from "lucide-react";
import { useEffect, useId, useState } from "react";
import { toast } from "sonner";

import { ConfirmDialog } from "@/components/confirm-dialog";
import { EmptyState, Panel } from "@/components/page";
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
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { ErrorAlert, Field, Input, Select } from "@/components/ui/field";
import { Switch } from "@/components/ui/switch";
import { type Schedule, ScheduleAction } from "@/gen/nethernode/v1/schedules_pb";
import { errorMessage } from "@/lib/api";
import { useCanManage } from "@/lib/session";
import { ago, until } from "@/lib/time";
import { useNow } from "@/lib/use-now";
import { cn } from "@/lib/utils";

import { scheduleClient, schedulesKey } from "./schedules-api";
import {
  fromCron,
  HOUR_STEPS,
  localTimeZone,
  parseTime,
  runTime,
  timeValue,
  timeZones,
  toCron,
  WEEKDAYS,
  type When,
} from "./when";

const ACTIONS: { action: ScheduleAction; label: string; name: string; icon: React.ReactNode }[] = [
  { action: ScheduleAction.BACKUP, label: "Back up", name: "Daily backup", icon: <History /> },
  { action: ScheduleAction.RESTART, label: "Restart", name: "Daily restart", icon: <RotateCcw /> },
  { action: ScheduleAction.BROADCAST, label: "Send a message", name: "Announcement", icon: <Megaphone /> },
  { action: ScheduleAction.COMMAND, label: "Run a command", name: "Command", icon: <SquareTerminal /> },
  { action: ScheduleAction.START, label: "Start", name: "Start", icon: <Play /> },
  { action: ScheduleAction.STOP, label: "Stop", name: "Stop", icon: <Square /> },
];

const actionInfo = (a: ScheduleAction) => ACTIONS.find((x) => x.action === a) ?? ACTIONS[0];

/** Minutes-before choices for restart and stop warnings. */
const WARNINGS = [15, 10, 5, 1];

interface ServerRef {
  id: string;
  name: string;
}

/** A server's schedules, or everyone's. */
export function SchedulesPanel({
  schedules,
  servers,
  serverId,
}: {
  schedules: Schedule[];
  servers: ServerRef[];
  serverId?: string;
}) {
  const client = useQueryClient();
  const canManage = useCanManage();
  const now = useNow();
  const [editing, setEditing] = useState<Schedule | "new" | null>(null);
  const [deleting, setDeleting] = useState<Schedule | null>(null);
  const [confirmRun, setConfirmRun] = useState<Schedule | null>(null);
  const refresh = () => client.invalidateQueries({ queryKey: schedulesKey });

  // Refresh just after the next run so its result shows without polling.
  const nextDue = schedules
    .filter((s) => s.enabled && s.nextRunMs !== undefined)
    .map((s) => Number(s.nextRunMs))
    .sort((a, b) => a - b)[0];
  useEffect(() => {
    if (nextDue === undefined) return;
    const wait = Math.min(Math.max(nextDue - Date.now(), 0) + 8_000, 2 ** 31 - 1);
    const id = setTimeout(() => void client.invalidateQueries({ queryKey: schedulesKey }), wait);
    return () => clearTimeout(id);
  }, [nextDue, client]);

  const save = useMutation({
    mutationFn: (s: Schedule) => scheduleClient.upsertSchedule({ schedule: s }),
    onSuccess: () => refresh(),
    onError: (e) => toast.error(errorMessage(e)),
  });
  const run = useMutation({
    mutationFn: (s: Schedule) => scheduleClient.runScheduleNow({ id: s.id }),
    onSuccess: (_, s) => {
      toast.success(`Running "${s.name}" now.`);
      setConfirmRun(null);
      setTimeout(() => void refresh(), 3_000);
    },
    onError: (e) => toast.error(errorMessage(e)),
  });
  const remove = useMutation({
    mutationFn: (s: Schedule) => scheduleClient.deleteSchedule({ id: s.id }),
    onSuccess: () => {
      setDeleting(null);
      void refresh();
    },
  });

  return (
    <Panel className="mb-6">
      <div className="mb-[15px] flex items-center justify-between gap-3">
        <h2 className="flex items-center gap-2 font-semibold text-label tracking-[-0.3px]">
          <CalendarClock className="size-[15px] text-text-muted" aria-hidden />
          Schedules
        </h2>
        {canManage && servers.length > 0 && (
          <Button size="mini" onClick={() => setEditing("new")}>
            <Plus />
            Add schedule
          </Button>
        )}
      </div>
      {schedules.length === 0 ? (
        <EmptyState icon={<CalendarClock />} title="Nothing scheduled" compact>
          Back up, restart, or send a message on a timetable, such as every night at 04:00.
        </EmptyState>
      ) : (
        <ul className="divide-y divide-divider">
          {schedules.map((s) => {
            const info = actionInfo(s.action);
            const failed = s.lastResult.startsWith("Failed");
            return (
              <li key={s.id} className="flex items-center gap-3 py-3 first:pt-0 last:pb-0">
                <span className="text-text-muted [&_svg]:size-4" aria-hidden>
                  {info?.icon}
                </span>
                <div className="min-w-0 flex-1">
                  <p className="truncate font-semibold text-label">
                    {s.name}
                    {!serverId && <span className="font-normal text-text-muted"> · {s.serverName}</span>}
                  </p>
                  <p className="text-meta text-text-muted">
                    {info?.label} · {s.description || s.cron}
                    {s.timezone !== localTimeZone() && ` (${s.timezone})`}
                  </p>
                  {s.lastResult && (
                    <p className={cn("truncate text-meta", failed ? "text-danger" : "text-text-muted")}>
                      {s.lastRunMs !== undefined && `${ago(s.lastRunMs, now)}: `}
                      {s.lastResult}
                    </p>
                  )}
                </div>
                <p className="shrink-0 text-right text-meta text-text-secondary below-sm:hidden">
                  {s.enabled
                    ? s.nextRunMs !== undefined
                      ? Number(s.nextRunMs) > now
                        ? `Next ${until(s.nextRunMs, now)}`
                        : "Due now"
                      : "—"
                    : "Off"}
                </p>
                {canManage && (
                  <>
                    <Switch
                      checked={s.enabled}
                      disabled={save.isPending}
                      aria-label={`${s.name}: ${s.enabled ? "turn off" : "turn on"}`}
                      onCheckedChange={(enabled) => save.mutate({ ...s, enabled })}
                    />
                    <DropdownMenu>
                      <DropdownMenuTrigger asChild>
                        <Button size="mini" variant="ghost" aria-label={`More actions for ${s.name}`}>
                          <Ellipsis />
                        </Button>
                      </DropdownMenuTrigger>
                      <DropdownMenuContent align="end">
                        <DropdownMenuItem onSelect={() => setEditing(s)}>
                          <Pencil />
                          Edit…
                        </DropdownMenuItem>
                        <DropdownMenuItem
                          onSelect={() =>
                            s.action === ScheduleAction.RESTART || s.action === ScheduleAction.STOP
                              ? setConfirmRun(s)
                              : run.mutate(s)
                          }
                        >
                          <Play />
                          Run now
                        </DropdownMenuItem>
                        <DropdownMenuSeparator />
                        <DropdownMenuItem className="text-danger" onSelect={() => setDeleting(s)}>
                          <Trash2 />
                          Delete…
                        </DropdownMenuItem>
                      </DropdownMenuContent>
                    </DropdownMenu>
                  </>
                )}
              </li>
            );
          })}
        </ul>
      )}
      {editing && (
        <ScheduleDialog
          schedule={editing === "new" ? null : editing}
          servers={servers}
          serverId={serverId}
          onClose={() => setEditing(null)}
        />
      )}
      {deleting && (
        <ConfirmDialog
          open
          onOpenChange={(o) => !o && setDeleting(null)}
          title={`Delete "${deleting.name}"?`}
          description="It stops running. Backups it already made are kept."
          confirmLabel="Delete schedule"
          pending={remove.isPending}
          error={remove.error ? errorMessage(remove.error) : undefined}
          onConfirm={() => remove.mutate(deleting)}
        />
      )}
      {confirmRun && (
        <ConfirmDialog
          open
          onOpenChange={(o) => !o && setConfirmRun(null)}
          title={`${actionInfo(confirmRun.action)?.label} ${confirmRun.serverName} now?`}
          description="Players are disconnected right away, without the usual countdown."
          confirmLabel={`${actionInfo(confirmRun.action)?.label} now`}
          pending={run.isPending}
          onConfirm={() => run.mutate(confirmRun)}
        />
      )}
    </Panel>
  );
}

function useDebounced<T>(value: T, ms: number): T {
  const [debounced, setDebounced] = useState(value);
  useEffect(() => {
    const id = setTimeout(() => setDebounced(value), ms);
    return () => clearTimeout(id);
  }, [value, ms]);
  return debounced;
}

/** Create or edit a schedule. */
function ScheduleDialog({
  schedule,
  servers,
  serverId,
  onClose,
}: {
  schedule: Schedule | null;
  servers: ServerRef[];
  serverId?: string | undefined;
  onClose: () => void;
}) {
  const client = useQueryClient();
  const ids = { server: useId(), action: useId(), when: useId(), tz: useId(), time: useId(), extra: useId() };
  const [server, setServer] = useState(schedule?.serverId ?? serverId ?? servers[0]?.id ?? "");
  const [action, setAction] = useState(schedule?.action ?? ScheduleAction.BACKUP);
  const [name, setName] = useState(schedule?.name ?? "");
  const [text, setText] = useState(schedule?.text ?? "");
  const [when, setWhen] = useState<When>(
    schedule ? fromCron(schedule.cron) : { kind: "daily", hour: 4, minute: 0 },
  );
  const [timezone, setTimezone] = useState(schedule?.timezone ?? localTimeZone());
  const [warn, setWarn] = useState<number[]>(schedule ? schedule.warnMinutes : [5, 1]);
  const editingBackup = schedule?.action === ScheduleAction.BACKUP;
  const [keepDaily, setKeepDaily] = useState(String(editingBackup ? schedule.keepDaily : 7));
  const [keepWeekly, setKeepWeekly] = useState(String(editingBackup ? schedule.keepWeekly : 4));
  const cron = toCron(when);
  const preview = useQuery({
    queryKey: ["schedules", "preview", useDebounced(cron, 300), timezone] as const,
    queryFn: ({ queryKey }) => scheduleClient.previewSchedule({ cron: queryKey[2], timezone: queryKey[3] }),
    placeholderData: keepPreviousData,
  });
  const info = actionInfo(action);
  const warns = action === ScheduleAction.RESTART || action === ScheduleAction.STOP;
  const backup = action === ScheduleAction.BACKUP;
  const needsText = action === ScheduleAction.COMMAND || action === ScheduleAction.BROADCAST;

  const save = useMutation({
    mutationFn: () =>
      scheduleClient.upsertSchedule({
        schedule: {
          id: schedule?.id ?? "",
          serverId: server,
          name: name.trim() || info?.name || "Schedule",
          cron,
          timezone,
          action,
          text: needsText ? text : "",
          warnMinutes: warns ? warn : [],
          keepDaily: backup ? Number(keepDaily) || 0 : 0,
          keepWeekly: backup ? Number(keepWeekly) || 0 : 0,
          enabled: schedule?.enabled ?? true,
        },
      }),
    onSuccess: () => {
      toast.success(schedule ? "Schedule saved." : "Schedule added.");
      void client.invalidateQueries({ queryKey: schedulesKey });
      onClose();
    },
  });

  const setTime = (value: string) => {
    const t = parseTime(value);
    if (t && when.kind !== "custom") setWhen({ ...when, ...t });
  };

  return (
    <Dialog open onOpenChange={(o) => !o && onClose()}>
      <DialogContent className="max-w-[520px]">
        <DialogHeader>
          <DialogTitle>{schedule ? "Edit schedule" : "Add a schedule"}</DialogTitle>
          <DialogDescription>
            Times follow the time zone you choose, including daylight saving.
          </DialogDescription>
        </DialogHeader>
        <form
          onSubmit={(e) => {
            e.preventDefault();
            save.mutate();
          }}
        >
          {save.error && <ErrorAlert>{errorMessage(save.error)}</ErrorAlert>}
          {!serverId && !schedule && (
            <Labeled id={ids.server} label="Server">
              <Select id={ids.server} value={server} onChange={(e) => setServer(e.target.value)}>
                {servers.map((s) => (
                  <option key={s.id} value={s.id}>
                    {s.name}
                  </option>
                ))}
              </Select>
            </Labeled>
          )}
          <div className="grid grid-cols-2 gap-x-3 below-sm:grid-cols-1">
            <Labeled id={ids.action} label="Action">
              <Select
                id={ids.action}
                value={action}
                onChange={(e) => setAction(Number(e.target.value) as ScheduleAction)}
              >
                {ACTIONS.map((a) => (
                  <option key={a.action} value={a.action}>
                    {a.label}
                  </option>
                ))}
              </Select>
            </Labeled>
            <Field
              label="Name"
              value={name}
              placeholder={info?.name}
              maxLength={60}
              onChange={(e) => setName(e.target.value)}
            />
          </div>
          {needsText && (
            <Field
              label={action === ScheduleAction.COMMAND ? "Command" : "Message"}
              value={text}
              required
              maxLength={500}
              placeholder={action === ScheduleAction.COMMAND ? "weather clear" : "Vote for the next event!"}
              hint={
                action === ScheduleAction.COMMAND
                  ? "Runs in the console, without the leading slash. Skipped while the server is off."
                  : "Shown in chat to everyone online. Skipped while the server is off."
              }
              inputClassName="font-mono"
              onChange={(e) => setText(e.target.value)}
            />
          )}

          <div className="grid grid-cols-2 gap-x-3 below-sm:grid-cols-1">
            <Labeled id={ids.when} label="Repeats">
              <Select
                id={ids.when}
                value={when.kind}
                onChange={(e) => {
                  const kind = e.target.value as When["kind"];
                  const { hour, minute } =
                    when.kind === "daily" || when.kind === "weekly" ? when : { hour: 4, minute: 0 };
                  setWhen(
                    kind === "daily"
                      ? { kind, hour, minute }
                      : kind === "weekly"
                        ? { kind, day: 0, hour, minute }
                        : kind === "hourly"
                          ? { kind, every: 6, minute: 0 }
                          : { kind, cron },
                  );
                }}
              >
                <option value="daily">Every day</option>
                <option value="weekly">Every week</option>
                <option value="hourly">Every few hours</option>
                <option value="custom">Custom (cron)</option>
              </Select>
            </Labeled>
            {when.kind === "daily" && (
              <Labeled id={ids.time} label="At">
                <Input
                  id={ids.time}
                  type="time"
                  required
                  value={timeValue(when.hour, when.minute)}
                  onChange={(e) => setTime(e.target.value)}
                />
              </Labeled>
            )}
            {when.kind === "weekly" && (
              <Labeled id={ids.extra} label="On">
                <Select
                  id={ids.extra}
                  value={when.day}
                  onChange={(e) => setWhen({ ...when, day: Number(e.target.value) })}
                >
                  {WEEKDAYS.map((d, i) => (
                    <option key={d} value={i}>
                      {d}
                    </option>
                  ))}
                </Select>
              </Labeled>
            )}
            {when.kind === "hourly" && (
              <Labeled id={ids.extra} label="Every">
                <Select
                  id={ids.extra}
                  value={when.every}
                  onChange={(e) => setWhen({ ...when, every: Number(e.target.value) })}
                >
                  {HOUR_STEPS.map((h) => (
                    <option key={h} value={h}>
                      {h === 1 ? "hour" : `${h} hours`}
                    </option>
                  ))}
                </Select>
              </Labeled>
            )}
            {when.kind === "custom" && (
              <Field
                label="Cron expression"
                value={when.cron}
                required
                placeholder="0 4 * * *"
                inputClassName="font-mono"
                hint="Minute, hour, day, month, weekday."
                onChange={(e) => setWhen({ kind: "custom", cron: e.target.value })}
              />
            )}
          </div>
          {when.kind === "weekly" && (
            <Labeled id={ids.time} label="At">
              <Input
                id={ids.time}
                type="time"
                required
                value={timeValue(when.hour, when.minute)}
                onChange={(e) => setTime(e.target.value)}
              />
            </Labeled>
          )}
          <Labeled id={ids.tz} label="Time zone">
            <Select id={ids.tz} value={timezone} onChange={(e) => setTimezone(e.target.value)}>
              {timeZones(timezone).map((z) => (
                <option key={z} value={z}>
                  {z.replaceAll("_", " ")}
                </option>
              ))}
            </Select>
          </Labeled>
          <div
            className="mb-[17px] rounded-md border border-divider bg-surface px-3 py-2.5 text-meta"
            aria-live="polite"
          >
            {preview.data?.error ? (
              <p className="text-danger">{preview.data.error}</p>
            ) : preview.data ? (
              <>
                <p className="font-semibold text-text-primary">{preview.data.description}</p>
                <p className="mt-1 text-text-muted">
                  Next:{" "}
                  {preview.data.nextRunsMs
                    .slice(0, 3)
                    .map((ms) => runTime(ms, timezone))
                    .join(" · ")}
                </p>
              </>
            ) : (
              <p className="text-text-muted">Working out the next runs…</p>
            )}
          </div>

          {warns && (
            <fieldset className="mb-[17px]">
              <legend className="mb-[7px] text-label text-text-secondary">Warn players before</legend>
              <div className="flex flex-wrap gap-4">
                {WARNINGS.map((m) => {
                  const id = `${ids.extra}-warn-${m}`;
                  return (
                    <div key={m} className="flex items-center gap-2">
                      <Checkbox
                        id={id}
                        checked={warn.includes(m)}
                        onCheckedChange={(v) =>
                          setWarn((w) => (v === true ? [...w, m] : w.filter((x) => x !== m)))
                        }
                      />
                      <label htmlFor={id} className="text-label">
                        {m} min
                      </label>
                    </div>
                  );
                })}
              </div>
              <p className="mt-1.5 text-meta text-text-muted">Shown in chat to everyone online.</p>
            </fieldset>
          )}
          {backup && (
            <>
              <div className="grid grid-cols-2 gap-x-3">
                <Field
                  label="Keep daily backups"
                  type="number"
                  min={0}
                  max={365}
                  value={keepDaily}
                  onChange={(e) => setKeepDaily(e.target.value)}
                />
                <Field
                  label="Keep weekly backups"
                  type="number"
                  min={0}
                  max={520}
                  value={keepWeekly}
                  onChange={(e) => setKeepWeekly(e.target.value)}
                />
              </div>
              <p className="-mt-2 mb-[17px] text-meta text-text-muted">
                After each run, scheduled backups beyond the newest one per day and per week are deleted. Use
                0 and 0 to keep everything. Manual backups are never deleted automatically.
              </p>
            </>
          )}
          <DialogFooter>
            <Button onClick={onClose}>Cancel</Button>
            <Button
              type="submit"
              variant="primary"
              disabled={save.isPending || !server || Boolean(preview.data?.error)}
            >
              {save.isPending ? "Saving…" : schedule ? "Save schedule" : "Add schedule"}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}

function Labeled({ id, label, children }: { id: string; label: string; children: React.ReactNode }) {
  return (
    <div className="mb-[17px]">
      <label htmlFor={id} className="mb-[7px] block text-label text-text-secondary">
        {label}
      </label>
      {children}
    </div>
  );
}
