import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { CircleCheck, ExternalLink } from "lucide-react";
import { useId, useState } from "react";
import { toast } from "sonner";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/field";
import { errorMessage } from "@/lib/api";

import { catalogClient } from "./api";
import { curseForgeStatusQuery } from "./queries";

/**
 * The CurseForge API key: lets NetherNode search CurseForge and install
 * from it. Never shown again once saved.
 */
export function CurseForgeKey() {
  const client = useQueryClient();
  const id = useId();
  const status = useQuery(curseForgeStatusQuery);
  const [key, setKey] = useState("");
  const save = useMutation({
    mutationFn: (value: string) => catalogClient.setCurseForgeKey({ key: value }),
    onSuccess: (_, value) => {
      setKey("");
      toast.success(value ? "CurseForge API key saved." : "CurseForge API key removed.");
      void client.invalidateQueries({ queryKey: ["catalog"] });
    },
  });
  return (
    <div className="w-full">
      {status.data ? (
        <div className="flex flex-wrap items-center gap-3">
          <span className="inline-flex items-center gap-1.5 text-label text-success">
            <CircleCheck className="size-4" aria-hidden />
            Key saved
          </span>
          <Button size="mini" variant="ghost" disabled={save.isPending} onClick={() => save.mutate("")}>
            Remove
          </Button>
        </div>
      ) : (
        <form
          className="flex flex-wrap gap-2"
          onSubmit={(e) => {
            e.preventDefault();
            if (key.trim()) save.mutate(key.trim());
          }}
        >
          <label htmlFor={id} className="sr-only">
            CurseForge API key
          </label>
          <Input
            id={id}
            type="password"
            autoComplete="off"
            spellCheck={false}
            placeholder="Paste your API key"
            className="min-w-[220px] flex-1 font-mono"
            value={key}
            onChange={(e) => setKey(e.target.value)}
          />
          <Button type="submit" disabled={!key.trim() || save.isPending}>
            {save.isPending ? "Checking…" : "Save"}
          </Button>
        </form>
      )}
      {save.isError && <p className="mt-1.5 text-meta text-danger">{errorMessage(save.error)}</p>}
      <a
        href="https://console.curseforge.com/"
        target="_blank"
        rel="noreferrer noopener"
        className="mt-2 inline-flex items-center gap-1 text-meta text-text-secondary underline underline-offset-2 hover:text-text-primary"
      >
        Get a free key at console.curseforge.com
        <ExternalLink className="size-3" aria-hidden />
      </a>
    </div>
  );
}
