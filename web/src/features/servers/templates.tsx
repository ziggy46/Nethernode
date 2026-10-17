import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { LayoutTemplate, Plus, Trash2 } from "lucide-react";
import { useState } from "react";
import { toast } from "sonner";

import { ConfirmDialog } from "@/components/confirm-dialog";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { ErrorAlert, Field } from "@/components/ui/field";
import type { ServerTemplate } from "@/gen/nethernode/v1/servers_pb";
import { errorMessage, serverClient } from "@/lib/api";
import { trackJob } from "@/lib/jobs";

import { MachineSelect, useMachines } from "./copy-server";
import { formatBytes, loaderName } from "./format";

export const templatesQuery = {
  queryKey: ["templates"],
  queryFn: async () => (await serverClient.listServerTemplates({})).templates,
};

/** Saved templates, on the first step of Create server. */
export function TemplatePicker({ onUse }: { onUse: (template: ServerTemplate) => void }) {
  const queryClient = useQueryClient();
  const templates = useQuery(templatesQuery).data ?? [];
  const [deleting, setDeleting] = useState<ServerTemplate | null>(null);
  const remove = useMutation({
    mutationFn: (id: string) => serverClient.deleteServerTemplate({ id }),
    onSuccess: () => {
      toast.success(`Deleted the template ${deleting?.name ?? ""}.`);
      setDeleting(null);
      void queryClient.invalidateQueries({ queryKey: templatesQuery.queryKey });
    },
  });
  if (templates.length === 0) return null;
  return (
    <section className="mt-5">
      <h3 className="mb-2 font-[550] text-label">Or start from a template</h3>
      <ul className="divide-y divide-divider rounded-md border border-divider">
        {templates.map((t) => (
          <li key={t.id} className="flex items-center gap-3 px-3 py-2.5">
            <LayoutTemplate className="size-4 shrink-0 text-text-muted" aria-hidden />
            <div className="min-w-0 flex-1">
              <p className="truncate font-[550] text-label">{t.name}</p>
              <p className="truncate text-meta text-text-muted">
                {loaderName(t.loader)} {t.mcVersion} · saved from {t.sourceName} ·{" "}
                {formatBytes(Number(t.sizeBytes))}
              </p>
            </div>
            <Button size="mini" onClick={() => onUse(t)}>
              Use
            </Button>
            <Button
              size="icon"
              variant="ghost"
              className="size-7"
              aria-label={`Delete the template ${t.name}`}
              onClick={() => setDeleting(t)}
            >
              <Trash2 className="size-[14px]" />
            </Button>
          </li>
        ))}
      </ul>
      <ConfirmDialog
        open={deleting !== null}
        onOpenChange={(open) => {
          if (!open) {
            setDeleting(null);
            remove.reset();
          }
        }}
        title={`Delete the template ${deleting?.name ?? ""}?`}
        description="Servers made from it aren't affected."
        confirmLabel={remove.isPending ? "Deleting…" : "Delete template"}
        pending={remove.isPending}
        error={remove.error ? errorMessage(remove.error) : undefined}
        onConfirm={() => deleting && remove.mutate(deleting.id)}
      />
    </section>
  );
}

/** Name and machine for a new server from `template`. */
export function FromTemplateDialog({
  template,
  onClose,
}: {
  template: ServerTemplate | null;
  onClose: () => void;
}) {
  const [name, setName] = useState("");
  const machines = useMachines(template !== null, "local");
  const create = useMutation({
    mutationFn: (t: ServerTemplate) =>
      serverClient.createServerFromTemplate({
        templateId: t.id,
        name: name.trim(),
        nodeId: machines.chosen === "local" ? "" : machines.chosen,
      }),
    onSuccess: ({ jobId }, t) => {
      trackJob(jobId, `Creating ${name.trim()} from ${t.name}…`);
      setName("");
      onClose();
    },
  });
  return (
    <Dialog
      open={template !== null}
      onOpenChange={(open) => {
        if (!open) {
          create.reset();
          onClose();
        }
      }}
    >
      <DialogContent>
        <DialogHeader>
          <DialogTitle>New server from {template?.name}</DialogTitle>
          <DialogDescription>
            It gets the template's software, mods, configuration and settings, and makes a new world when it
            first starts. It starts out stopped.
          </DialogDescription>
        </DialogHeader>
        {create.isError && <ErrorAlert>{errorMessage(create.error)}</ErrorAlert>}
        <form
          onSubmit={(e) => {
            e.preventDefault();
            if (template && name.trim()) create.mutate(template);
          }}
        >
          <Field
            label="Name"
            placeholder="My new server"
            value={name}
            maxLength={64}
            autoFocus
            onChange={(e) => setName(e.target.value)}
          />
          <MachineSelect machines={machines} />
          <DialogFooter>
            <Button onClick={onClose}>Cancel</Button>
            <Button
              type="submit"
              variant="primary"
              disabled={!name.trim() || !machines.chosen || create.isPending}
            >
              <Plus />
              {create.isPending ? "Starting…" : "Create server"}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
