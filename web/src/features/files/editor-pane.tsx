import { Code, ConnectError } from "@connectrpc/connect";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { ArrowLeft, Download, FileWarning, Save, TriangleAlert, X } from "lucide-react";
import { lazy, Suspense, useRef, useState } from "react";
import { toast } from "sonner";

import { EmptyState } from "@/components/page";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { type Server, ServerStatus } from "@/gen/nethernode/v1/servers_pb";
import { errorMessage } from "@/lib/api";
import { cn } from "@/lib/utils";

import { fileClient } from "./api";
import type { CodeEditorHandle } from "./code-editor";
import { baseName, languageFor, runningEditWarning } from "./paths";
import { filesKey } from "./queries";

const CodeEditor = lazy(() => import("./code-editor"));

export function downloadUrl(serverId: string, path: string): string {
  return `/api/files/download?${new URLSearchParams({ server: serverId, path })}`;
}

/** The right-hand pane: one open file. */
export function EditorPane({
  server,
  path,
  onClose,
  onDirtyChange,
}: {
  server: Server;
  path: string;
  onClose: () => void;
  onDirtyChange: (dirty: boolean) => void;
}) {
  const client = useQueryClient();
  const readKey = [...filesKey(server.id), "read", path] as const;
  const file = useQuery({
    queryKey: readKey,
    queryFn: () => fileClient.readFile({ serverId: server.id, path }),
    staleTime: Number.POSITIVE_INFINITY,
    gcTime: 0,
    retry: false,
  });
  const editor = useRef<CodeEditorHandle>(null);
  const [dirty, setDirty] = useState(false);
  const [conflict, setConflict] = useState(false);
  // Bumped to remount the editor with fresh content after a reload.
  const [generation, setGeneration] = useState(0);

  const markDirty = (next: boolean) => {
    setDirty(next);
    onDirtyChange(next);
  };

  const save = useMutation({
    mutationFn: async (force: boolean) => {
      const text = editor.current?.value() ?? "";
      const response = await fileClient.writeFile({
        serverId: server.id,
        path,
        content: new TextEncoder().encode(text),
        expectedVersion: force ? undefined : file.data?.version,
      });
      return { version: response.version, text };
    },
    onSuccess: ({ version, text }) => {
      editor.current?.markSaved();
      client.setQueryData(readKey, (old) =>
        old ? { ...old, version, content: new TextEncoder().encode(text) } : old,
      );
      void client.invalidateQueries({ queryKey: [...filesKey(server.id), "list"] });
      void client.invalidateQueries({ queryKey: [...filesKey(server.id), "properties"] });
      toast.success(`Saved ${baseName(path)}.`);
    },
    onError: (err) => {
      if (ConnectError.from(err).code === Code.Aborted) setConflict(true);
      else toast.error(errorMessage(err));
    },
  });

  const warning =
    server.status === ServerStatus.RUNNING || path === "server.properties" ? runningEditWarning(path) : null;
  const tooLarge = file.isError && ConnectError.from(file.error).code === Code.FailedPrecondition;

  return (
    <section
      aria-label={`Editing ${path}`}
      className="flex h-[640px] min-w-0 flex-col overflow-hidden rounded-[9px] border border-border bg-console below-lg:h-[70vh]"
    >
      <div className="flex items-center gap-2 border-border border-b bg-surface px-[13px] py-[9px]">
        <Button
          size="icon"
          variant="ghost"
          className="hidden size-7 below-lg:grid"
          aria-label="Back to files"
          onClick={onClose}
        >
          <ArrowLeft className="size-[15px]" />
        </Button>
        <div className="min-w-0 flex-1">
          <p className="truncate font-mono text-label" title={path}>
            {path}
          </p>
          <p className={cn("text-meta", dirty ? "text-warning" : "text-text-muted")} role="status">
            {dirty ? "Unsaved changes" : file.isSuccess ? "Saved" : " "}
          </p>
        </div>
        <Button size="mini" asChild>
          <a href={downloadUrl(server.id, path)} download>
            <Download />
            Download
          </a>
        </Button>
        <Button
          size="mini"
          variant="primary"
          disabled={!dirty || save.isPending || !file.data?.isText}
          onClick={() => save.mutate(false)}
          title="Save (Ctrl+S / ⌘S)"
        >
          <Save />
          {save.isPending ? "Saving…" : "Save"}
        </Button>
        <Button
          size="icon"
          variant="ghost"
          className="size-7 below-lg:hidden"
          aria-label="Close file"
          onClick={onClose}
        >
          <X className="size-[15px]" />
        </Button>
      </div>
      {warning && (
        <p className="flex items-start gap-2 border-border border-b bg-warning-subtle px-[13px] py-2 text-meta text-warning">
          <TriangleAlert className="mt-px size-[13px] shrink-0" aria-hidden />
          {warning}
        </p>
      )}
      <div className="min-h-0 flex-1">
        {file.isPending ? (
          <div className="h-full animate-pulse bg-console" aria-busy />
        ) : file.isError ? (
          <div className="grid h-full place-items-center p-6">
            <EmptyState
              compact
              icon={<FileWarning />}
              title={tooLarge ? "Too large to edit here" : "Couldn't open this file"}
              action={
                tooLarge && (
                  <Button asChild>
                    <a href={downloadUrl(server.id, path)} download>
                      <Download />
                      Download instead
                    </a>
                  </Button>
                )
              }
            >
              {tooLarge ? "The editor opens files up to 5 MB." : errorMessage(file.error)}
            </EmptyState>
          </div>
        ) : !file.data.isText ? (
          <div className="grid h-full place-items-center p-6">
            <EmptyState
              compact
              icon={<FileWarning />}
              title="This isn't a text file"
              action={
                <Button asChild>
                  <a href={downloadUrl(server.id, path)} download>
                    <Download />
                    Download
                  </a>
                </Button>
              }
            >
              Binary files (worlds, jars, images) can be downloaded but not edited here.
            </EmptyState>
          </div>
        ) : (
          <Suspense fallback={<div className="h-full animate-pulse bg-console" aria-busy />}>
            <CodeEditor
              key={`${path}:${generation}`}
              ref={editor}
              initial={new TextDecoder().decode(file.data.content)}
              language={languageFor(path)}
              label={`Contents of ${path}`}
              onDirtyChange={markDirty}
              onSave={() => {
                if (!save.isPending) save.mutate(false);
              }}
            />
          </Suspense>
        )}
      </div>

      <Dialog open={conflict} onOpenChange={setConflict}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>The file changed on disk</DialogTitle>
            <DialogDescription>
              {baseName(path)} was changed after you opened it, probably by the server. Saving now would
              replace those changes with yours.
            </DialogDescription>
          </DialogHeader>
          <DialogFooter>
            <Button
              onClick={async () => {
                setConflict(false);
                await file.refetch();
                markDirty(false);
                setGeneration((g) => g + 1);
              }}
            >
              Reload (discard my edits)
            </Button>
            <Button
              variant="danger"
              onClick={() => {
                setConflict(false);
                save.mutate(true);
              }}
            >
              Overwrite with mine
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </section>
  );
}
