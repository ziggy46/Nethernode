import { useInfiniteQuery, useQuery } from "@tanstack/react-query";
import { ClipboardList, Lock, TriangleAlert } from "lucide-react";
import { useId, useState } from "react";

import { EmptyState, Footer, PageHeader } from "@/components/page";
import { Button } from "@/components/ui/button";
import { Select } from "@/components/ui/field";
import { Role } from "@/gen/nethernode/v1/auth_pb";
import { errorMessage } from "@/lib/api";
import { healthQuery } from "@/lib/health";
import { useCurrentUser } from "@/lib/session";
import { ago, dateTime } from "@/lib/time";
import { useDocumentTitle } from "@/lib/use-document-title";
import { useNow } from "@/lib/use-now";

import { serversQuery } from "../servers/queries";
import { auditClient } from "./api";
import { actionLabel, CATEGORIES, detailPairs } from "./labels";

/** Who did what, and when. Administrators and owners. */
export function AuditPage() {
  useDocumentTitle("Audit log");
  const health = useQuery(healthQuery);
  const { role } = useCurrentUser();
  const allowed = role >= Role.ADMIN;
  const ids = { category: useId(), user: useId(), server: useId() };
  const [category, setCategory] = useState("");
  const [userId, setUserId] = useState("");
  const [serverId, setServerId] = useState("");
  const servers = useQuery({ ...serversQuery, enabled: allowed });
  const log = useInfiniteQuery({
    queryKey: ["audit", category, userId, serverId] as const,
    queryFn: ({ pageParam }) =>
      auditClient.listAudit({
        category,
        userId: userId || undefined,
        serverId: serverId || undefined,
        beforeId: pageParam,
        limit: 50,
      }),
    initialPageParam: undefined as string | undefined,
    getNextPageParam: (last) => last.nextBeforeId,
    enabled: allowed,
  });
  const now = useNow();
  const entries = log.data?.pages.flatMap((p) => p.entries) ?? [];
  const actors = log.data?.pages[0]?.actors ?? [];
  const filtered = Boolean(category || userId || serverId);

  return (
    <>
      <PageHeader
        eyebrow="Accountability"
        title="Audit log"
        description="Every change made through NetherNode: who, what, and when."
      />
      {!allowed ? (
        <div className="rounded-lg border border-divider border-dashed bg-surface-subtle">
          <EmptyState icon={<Lock />} title="The audit log is for administrators">
            Ask an administrator or owner if you need to know who changed something.
          </EmptyState>
        </div>
      ) : (
        <>
          <div className="mb-[17px] grid grid-cols-3 gap-3 below-md:grid-cols-1">
            <Filter id={ids.category} label="What">
              <Select id={ids.category} value={category} onChange={(e) => setCategory(e.target.value)}>
                {CATEGORIES.map((c) => (
                  <option key={c.value} value={c.value}>
                    {c.label}
                  </option>
                ))}
              </Select>
            </Filter>
            <Filter id={ids.user} label="Who">
              <Select id={ids.user} value={userId} onChange={(e) => setUserId(e.target.value)}>
                <option value="">Anyone</option>
                {actors.map((a) => (
                  <option key={a.id} value={a.id}>
                    {a.username}
                  </option>
                ))}
              </Select>
            </Filter>
            <Filter id={ids.server} label="Server">
              <Select id={ids.server} value={serverId} onChange={(e) => setServerId(e.target.value)}>
                <option value="">Any server</option>
                {(servers.data ?? []).map((s) => (
                  <option key={s.id} value={s.id}>
                    {s.name}
                  </option>
                ))}
              </Select>
            </Filter>
          </div>
          {log.isError ? (
            <div className="rounded-lg border border-divider border-dashed bg-surface-subtle">
              <EmptyState
                icon={<TriangleAlert />}
                title="Couldn't load the audit log"
                action={<Button onClick={() => void log.refetch()}>Try again</Button>}
              >
                {errorMessage(log.error)}
              </EmptyState>
            </div>
          ) : log.isPending ? (
            <div className="h-64 animate-pulse rounded-lg bg-surface-subtle" role="status" aria-busy>
              <span className="sr-only">Loading the audit log…</span>
            </div>
          ) : entries.length === 0 ? (
            <div className="rounded-lg border border-divider border-dashed bg-surface-subtle">
              <EmptyState icon={<ClipboardList />} title={filtered ? "Nothing matches" : "Nothing yet"}>
                {filtered ? "Try a different filter." : "Changes made through NetherNode appear here."}
              </EmptyState>
            </div>
          ) : (
            <>
              <div className="overflow-x-auto rounded-lg border border-border">
                <table className="w-full min-w-[640px] text-label">
                  <thead className="bg-surface-subtle text-left text-meta text-text-muted">
                    <tr>
                      {["When", "Who", "What", "Details"].map((h) => (
                        <th key={h} scope="col" className="border-divider border-b px-3 py-2.5 font-medium">
                          {h}
                        </th>
                      ))}
                    </tr>
                  </thead>
                  <tbody>
                    {entries.map((e) => {
                      const pairs = detailPairs(e.detailsJson);
                      return (
                        <tr key={e.id} className="border-divider border-b align-top last:border-b-0">
                          <td className="whitespace-nowrap px-3 py-2.5 text-text-secondary">
                            <span title={dateTime(e.createdAtMs)}>{ago(e.createdAtMs, now)}</span>
                          </td>
                          <td className="whitespace-nowrap px-3 py-2.5">
                            {e.username || <span className="text-text-muted">NetherNode</span>}
                          </td>
                          <td className="px-3 py-2.5">
                            <span className={e.action === "auth.login_failed" ? "text-warning" : undefined}>
                              {actionLabel(e.action)}
                            </span>
                            {e.targetName && <span className="font-semibold"> · {e.targetName}</span>}
                          </td>
                          <td className="px-3 py-2.5 text-meta text-text-muted">
                            {pairs.length === 0
                              ? "—"
                              : pairs.map(([k, v]) => (
                                  <span key={k} className="mr-3 inline-block">
                                    {k}: <span className="text-text-secondary">{v}</span>
                                  </span>
                                ))}
                          </td>
                        </tr>
                      );
                    })}
                  </tbody>
                </table>
              </div>
              {log.hasNextPage && (
                <div className="mt-4 flex justify-center">
                  <Button disabled={log.isFetchingNextPage} onClick={() => void log.fetchNextPage()}>
                    {log.isFetchingNextPage ? "Loading…" : "Load older entries"}
                  </Button>
                </div>
              )}
            </>
          )}
        </>
      )}
      <Footer version={health.data?.version} />
    </>
  );
}

function Filter({ id, label, children }: { id: string; label: string; children: React.ReactNode }) {
  return (
    <div>
      <label htmlFor={id} className="mb-[7px] block text-meta text-text-muted">
        {label}
      </label>
      {children}
    </div>
  );
}
