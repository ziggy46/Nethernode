import { useQuery } from "@tanstack/react-query";
import { X } from "lucide-react";
import { useEffect, useId, useState } from "react";

import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import type { LogMatch } from "@/gen/nethernode/v1/servers_pb";
import { errorMessage, serverClient } from "@/lib/api";

const dateFormat = new Intl.DateTimeFormat(undefined, { dateStyle: "medium" });

/** "Current log", or the day a dated log is from ("2026-10-15-2.log.gz"). */
function fileLabel(file: string, modifiedMs: bigint): string {
  if (file === "latest.log") return `Latest run (to ${dateFormat.format(new Date(Number(modifiedMs)))})`;
  const m = /^(\d{4})-(\d{2})-(\d{2})-(\d+)\.log\.gz$/.exec(file);
  if (!m) return file;
  const day = dateFormat.format(new Date(Number(m[1]), Number(m[2]) - 1, Number(m[3])));
  return m[4] === "1" ? day : `${day}, run ${m[4]}`;
}

/** `text` with each case-insensitive occurrence of `needle` marked. */
function Highlighted({ text, needle }: { text: string; needle: string }) {
  if (!needle) return <>{text}</>;
  const lower = text.toLowerCase();
  const target = needle.toLowerCase();
  const parts: React.ReactNode[] = [];
  let at = 0;
  for (let i = lower.indexOf(target); i !== -1; i = lower.indexOf(target, i + target.length)) {
    parts.push(text.slice(at, i));
    parts.push(
      <mark key={i} className="rounded-xs bg-primary-subtle text-text-primary">
        {text.slice(i, i + target.length)}
      </mark>,
    );
    at = i + target.length;
  }
  parts.push(text.slice(at));
  return <>{parts}</>;
}

/** Waits for typing to settle. */
function useSettled<T>(value: T, ms = 350): T {
  const [settled, setSettled] = useState(value);
  useEffect(() => {
    const timer = setTimeout(() => setSettled(value), ms);
    return () => clearTimeout(timer);
  }, [value, ms]);
  return settled;
}

/**
 * Matches in the server's saved logs (older runs included), under the
 * console's search bar. With an empty search, it lists chat.
 */
export function OlderLogs({
  serverId,
  query,
  onClose,
}: {
  serverId: string;
  query: string;
  onClose: () => void;
}) {
  const chatId = useId();
  const [chatOnly, setChatOnly] = useState(false);
  const needle = useSettled(query.trim());
  const chat = chatOnly || !needle;
  const result = useQuery({
    queryKey: ["servers", "logs", serverId, needle, chat],
    queryFn: () => serverClient.searchServerLogs({ id: serverId, query: needle, chatOnly: chat }),
    staleTime: 0,
  });
  const data = result.data;
  const groups: { file: string; label: string; lines: LogMatch[] }[] = [];
  for (const m of data?.matches ?? []) {
    const last = groups[groups.length - 1];
    if (last?.file === m.file) last.lines.push(m);
    else groups.push({ file: m.file, label: fileLabel(m.file, m.fileModifiedMs), lines: [m] });
  }

  return (
    <div className="border-border border-b bg-surface-subtle">
      <div className="flex flex-wrap items-center gap-3 px-[17px] py-2 text-meta">
        <span className="font-[550] text-text-primary">
          {needle ? <>Older logs: “{needle}”</> : "Chat in older logs"}
        </span>
        <span className="text-text-muted" aria-live="polite">
          {result.isFetching
            ? "Searching…"
            : data
              ? `${data.matches.length}${data.truncated ? "+" : ""} ${data.matches.length === 1 ? "line" : "lines"} in ${data.filesSearched} ${data.filesSearched === 1 ? "file" : "files"}`
              : ""}
        </span>
        <span className="ml-auto flex items-center gap-3">
          <span className="flex items-center gap-1.5">
            <Checkbox
              id={chatId}
              checked={chat}
              disabled={!needle}
              onCheckedChange={(v) => setChatOnly(v === true)}
            />
            <label htmlFor={chatId} className="text-text-secondary">
              Chat only
            </label>
          </span>
          <Button
            size="icon"
            variant="ghost"
            className="size-6"
            aria-label="Close older logs"
            onClick={onClose}
          >
            <X className="size-[13px]" />
          </Button>
        </span>
      </div>
      <div className="max-h-[300px] overflow-y-auto px-[17px] pb-3">
        {result.isError ? (
          <p className="text-danger text-meta">{errorMessage(result.error)}</p>
        ) : data && groups.length === 0 ? (
          <p className="text-meta text-text-muted">
            {data.filesTotal === 0
              ? "This server has no saved logs yet."
              : needle
                ? "No matches. Minecraft keeps logs from earlier runs in the logs folder; this searched all of them."
                : "No chat found in the saved logs."}
          </p>
        ) : (
          groups.map((g) => (
            <section key={g.file} className="mt-2 first:mt-0">
              <h3 className="sticky top-0 bg-surface-subtle py-1 text-meta text-text-muted">{g.label}</h3>
              <ul className="font-mono text-meta text-text-secondary">
                {g.lines.map((m) => (
                  <li key={m.line} className="break-words py-px">
                    <Highlighted text={m.text} needle={needle} />
                  </li>
                ))}
              </ul>
            </section>
          ))
        )}
        {data?.truncated && (
          <p className="mt-2 text-meta text-text-muted">
            Showing the newest {data.matches.length} lines. Narrow the search to see older ones.
          </p>
        )}
      </div>
    </div>
  );
}
