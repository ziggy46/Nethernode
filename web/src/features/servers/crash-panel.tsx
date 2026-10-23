import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Stethoscope, Wrench } from "lucide-react";

import { Panel, PanelHeader } from "@/components/page";
import { Button } from "@/components/ui/button";
import { ErrorAlert } from "@/components/ui/field";
import type { CrashCause, Server } from "@/gen/nethernode/v1/servers_pb";
import { errorMessage, serverClient } from "@/lib/api";
import { withToast } from "@/lib/toast";

const crashKey = (id: string) => ["servers", "crash", id] as const;

/** Why a crashed server stopped, from its crash report, with a fix to click
 * where NetherNode has one. Shows nothing when it can't tell. */
export function CrashPanel({ server, canManage }: { server: Server; canManage: boolean }) {
  const queryClient = useQueryClient();
  const diagnosis = useQuery({
    queryKey: crashKey(server.id),
    queryFn: () => serverClient.getCrashDiagnosis({ id: server.id }),
    enabled: canManage,
    staleTime: 60_000,
  });
  const fix = useMutation({
    mutationFn: (kind: string) => serverClient.fixCrash({ id: server.id, kind }),
    onSuccess: ({ message }) => {
      withToast((toast) => toast.success(message));
      void queryClient.invalidateQueries({ queryKey: ["servers"] });
      void queryClient.invalidateQueries({ queryKey: ["mods", server.id] });
    },
  });
  const causes = diagnosis.data?.causes ?? [];
  if (!canManage || causes.length === 0) return null;

  return (
    <Panel className="border-danger/30">
      <PanelHeader title="Why it stopped" icon={<Stethoscope />} />
      <p className="mb-4 text-meta text-text-muted">
        From <span className="font-mono">{diagnosis.data?.source}</span>.
      </p>
      {fix.isError && <ErrorAlert>{errorMessage(fix.error)}</ErrorAlert>}
      <ul className="flex flex-col gap-4">
        {causes.map((cause) => (
          <Cause
            key={cause.kind}
            cause={cause}
            pending={fix.isPending && fix.variables === cause.kind}
            disabled={fix.isPending}
            onFix={() => fix.mutate(cause.kind)}
          />
        ))}
      </ul>
    </Panel>
  );
}

function Cause({
  cause,
  pending,
  disabled,
  onFix,
}: {
  cause: CrashCause;
  pending: boolean;
  disabled: boolean;
  onFix: () => void;
}) {
  return (
    <li>
      <p className="text-label">{cause.summary}</p>
      {cause.mods.length > 0 && (
        <ul className="mt-2 flex flex-col gap-1">
          {cause.mods.map((m) => (
            <li key={m.modId} className="text-meta">
              <span className="font-[550] text-text-primary">{m.name || m.modId}</span>
              {m.file ? <span className="font-mono text-text-muted"> · mods/{m.file}</span> : null}
              {m.detail ? <span className="text-text-secondary"> · {m.detail}</span> : null}
              {!m.file && <span className="text-text-muted"> · not found in mods/</span>}
            </li>
          ))}
        </ul>
      )}
      {cause.fix && (
        <Button size="mini" variant="primary" className="mt-3" disabled={disabled} onClick={onFix}>
          <Wrench />
          {pending ? "Fixing…" : cause.fix}
        </Button>
      )}
    </li>
  );
}
