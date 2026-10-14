import { queryOptions, useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { RotateCcw, Trash2 } from "lucide-react";
import { useRef, useState, useSyncExternalStore } from "react";
import { toast } from "sonner";

import { Button } from "@/components/ui/button";
import { CSRF_HEADER_NAME, csrfToken } from "@/lib/api";
import { bytes } from "./format";
import { enqueue, getUploads, subscribe } from "./store";

interface Unfinished {
  id: string;
  server_id: string;
  dir: string;
  filename: string;
  size: number;
  received: number;
  status: string;
}

export const unfinishedKey = ["uploads", "unfinished"] as const;

const unfinishedQuery = queryOptions({
  queryKey: unfinishedKey,
  queryFn: async (): Promise<Unfinished[]> => {
    const response = await fetch("/api/uploads", { credentials: "same-origin" });
    if (!response.ok) throw new Error(`couldn't list uploads (${response.status})`);
    return (await response.json()) as Unfinished[];
  },
});

/**
 * Uploads to this server that stopped partway (the page was closed, say).
 * Choosing the same file again continues from where it stopped.
 */
export function UnfinishedUploads({ serverId, serverName }: { serverId: string; serverName: string }) {
  const client = useQueryClient();
  const list = useQuery(unfinishedQuery);
  const items = useSyncExternalStore(subscribe, getUploads);
  const picker = useRef<HTMLInputElement>(null);
  const [resuming, setResuming] = useState<Unfinished | null>(null);
  const discard = useMutation({
    mutationFn: async (upload: Unfinished) => {
      const csrf = await csrfToken();
      const response = await fetch(`/api/uploads/${upload.id}`, {
        method: "DELETE",
        credentials: "same-origin",
        headers: { "Tus-Resumable": "1.0.0", ...(csrf ? { [CSRF_HEADER_NAME]: csrf } : {}) },
      });
      if (!response.ok && response.status !== 410) throw new Error(await response.text());
    },
    onSettled: () => client.invalidateQueries({ queryKey: unfinishedKey }),
    onError: (error) => toast.error(error.message),
  });

  // Ones this tab is already sending aren't "unfinished".
  const sending = new Set(items.filter((i) => i.status !== "cancelled").map((i) => `${i.dir}/${i.name}`));
  const stalled = (list.data ?? []).filter(
    (u) => u.server_id === serverId && u.status === "uploading" && !sending.has(`${u.dir}/${u.filename}`),
  );
  if (stalled.length === 0) return null;

  return (
    <div className="mb-4 rounded-md border border-warning/30 bg-warning-subtle px-3.5 py-2.5">
      <input
        ref={picker}
        type="file"
        hidden
        aria-label="Choose the file to resume"
        onChange={(e) => {
          const file = e.target.files?.[0];
          e.target.value = "";
          const upload = resuming;
          setResuming(null);
          if (!file || !upload) return;
          if (file.name !== upload.filename || file.size !== upload.size) {
            toast.error(
              `That isn't ${upload.filename} (${bytes(upload.size)}). Choose the same file to resume.`,
            );
            return;
          }
          enqueue({ serverId, serverName, dir: upload.dir }, [file], {
            conflict: "rename",
            extract: false,
            resumeUrl: `/api/uploads/${upload.id}`,
          });
        }}
      />
      <p className="mb-1.5 text-label text-warning">
        {stalled.length === 1 ? "An upload didn't finish." : `${stalled.length} uploads didn't finish.`}{" "}
        Choose the same file to continue where it stopped.
      </p>
      <ul className="flex flex-col gap-1.5">
        {stalled.map((u) => (
          <li key={u.id} className="flex flex-wrap items-center gap-2 text-meta">
            <span
              className="min-w-0 flex-1 truncate text-text-primary"
              title={`${u.dir ? `${u.dir}/` : ""}${u.filename}`}
            >
              {u.dir ? `${u.dir}/` : ""}
              {u.filename}
              <span className="text-text-muted">
                {" "}
                · {bytes(u.received)} of {bytes(u.size)}
              </span>
            </span>
            <Button
              size="mini"
              onClick={() => {
                setResuming(u);
                picker.current?.click();
              }}
            >
              <RotateCcw />
              Resume
            </Button>
            <Button
              size="mini"
              variant="ghost"
              disabled={discard.isPending}
              onClick={() => discard.mutate(u)}
            >
              <Trash2 />
              Discard
            </Button>
          </li>
        ))}
      </ul>
    </div>
  );
}
