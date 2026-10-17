/**
 * The live console: xterm.js fed by the console WebSocket, with search,
 * copy, a scroll lock, command history, and command completion. Loaded on
 * demand (see console-panel.tsx) so xterm stays out of the app shell.
 */
import "@xterm/xterm/css/xterm.css";

import { FitAddon } from "@xterm/addon-fit";
import { SearchAddon } from "@xterm/addon-search";
import { Terminal } from "@xterm/xterm";
import {
  ArrowDown,
  ArrowRight,
  ArrowUp,
  ClipboardCopy,
  Eraser,
  Search,
  SquareTerminal,
  X,
} from "lucide-react";
import { useEffect, useMemo, useRef, useState } from "react";
import { toast } from "sonner";

import { ServerStatusBadge } from "@/components/status-badge";
import { Button } from "@/components/ui/button";
import { type Server, ServerStatus } from "@/gen/nethernode/v1/servers_pb";
import { useTheme } from "@/lib/theme";
import { cn } from "@/lib/utils";

import { type ConsoleColors, colorize, completions, fg, type Rgb, sectionToAnsi } from "./format";
import { type ConsoleFrame, type SocketState, useConsoleSocket } from "./use-console-socket";

const RESET = "\x1b[0m";
const HIDE_CURSOR = "\x1b[?25l";
const HISTORY_LIMIT = 100;

/** Resolves a design token to `#rrggbb` (xterm needs plain colors). */
function tokenHex(name: string): string {
  const probe = document.createElement("span");
  probe.style.color = `var(--color-${name})`;
  document.body.append(probe);
  const computed = getComputedStyle(probe).color;
  probe.remove();
  const ctx = document.createElement("canvas").getContext("2d");
  if (!ctx) return computed;
  ctx.fillStyle = computed;
  return ctx.fillStyle;
}

function toRgb(hex: string): Rgb {
  const n = Number.parseInt(hex.replace("#", "").slice(0, 6), 16);
  return [(n >> 16) & 255, (n >> 8) & 255, n & 255];
}

function readPalette() {
  const ansi = ["black", "red", "green", "yellow", "blue", "magenta", "cyan", "white"] as const;
  const theme: Record<string, string> = {
    background: tokenHex("console"),
    foreground: tokenHex("console-text"),
    selectionBackground: tokenHex("console-selection"),
    cursor: tokenHex("console"),
  };
  for (const color of ansi) {
    const bright = `bright${color[0]?.toUpperCase()}${color.slice(1)}`;
    theme[color] = tokenHex(`ansi-${color}`);
    theme[bright] = tokenHex(`ansi-bright-${color}`);
  }
  const colors: ConsoleColors = {
    dim: toRgb(tokenHex("console-dim")),
    success: toRgb(tokenHex("console-success")),
    warning: toRgb(tokenHex("console-warning")),
    danger: toRgb(tokenHex("console-danger")),
    prompt: toRgb(tokenHex("primary")),
  };
  const search = { match: tokenHex("primary-subtle"), active: tokenHex("primary") };
  return { theme, colors, search };
}

function useHistory(serverId: string) {
  const key = `nn.console.history.${serverId}`;
  const load = (): string[] => {
    try {
      const parsed: unknown = JSON.parse(localStorage.getItem(key) ?? "[]");
      return Array.isArray(parsed) ? parsed.filter((x): x is string => typeof x === "string") : [];
    } catch {
      return [];
    }
  };
  const [entries, setEntries] = useState(load);
  const push = (command: string) => {
    setEntries((prev) => {
      const next = [...prev.filter((c) => c !== command), command].slice(-HISTORY_LIMIT);
      try {
        localStorage.setItem(key, JSON.stringify(next));
      } catch {
        // Storage may be unavailable (private mode); history is a convenience.
      }
      return next;
    });
  };
  return { entries, push };
}

const STATE_LABEL: Record<SocketState, string> = {
  connecting: "Connecting…",
  live: "Live",
  reconnecting: "Reconnecting…",
};

export default function ConsoleView({ server }: { server: Server }) {
  const host = useRef<HTMLDivElement>(null);
  const term = useRef<Terminal | null>(null);
  const searcher = useRef<SearchAddon | null>(null);
  const theme = useTheme();
  // biome-ignore lint/correctness/useExhaustiveDependencies: the tokens it reads change with the theme.
  const palette = useMemo(readPalette, [theme]);
  const firstPalette = useRef(palette);
  const [hasOutput, setHasOutput] = useState(false);
  const [following, setFollowing] = useState(true);
  const [unseen, setUnseen] = useState(0);
  const [searchOpen, setSearchOpen] = useState(false);
  const [query, setQuery] = useState("");
  const [results, setResults] = useState<{ index: number; count: number } | null>(null);
  const followingRef = useRef(true);

  const write = (text: string) => {
    const t = term.current;
    if (!t) return;
    t.write(text);
    if (!followingRef.current) setUnseen((n) => n + 1);
  };

  const echo = (command: string) => `${fg(palette.colors.prompt)}› ${command}${RESET}\r\n`;

  const { state, send } = useConsoleSocket(server.id, (frame: ConsoleFrame) => {
    const t = term.current;
    if (!t) return;
    switch (frame.type) {
      case "backlog":
        t.reset();
        t.write(HIDE_CURSOR);
        setUnseen(0);
        if (frame.lines.length > 0) {
          t.write(`${frame.lines.map((l) => colorize(l, palette.colors)).join("\r\n")}\r\n`);
          setHasOutput(true);
        }
        break;
      case "lines":
        write(`${frame.lines.map((l) => colorize(l, palette.colors)).join("\r\n")}\r\n`);
        setHasOutput(true);
        break;
      case "skipped":
        write(
          `${fg(palette.colors.dim)}… ${frame.count} lines skipped (output was too fast to show)${RESET}\r\n`,
        );
        break;
      // Each command prints with its result when the reply arrives, so
      // quick successive commands don't interleave.
      case "reply": {
        const output = frame.output.trimEnd();
        const result = output ? `${sectionToAnsi(output).replaceAll("\n", "\r\n")}${RESET}\r\n` : "";
        write(`${echo(frame.command)}${result}`);
        break;
      }
      case "error":
        write(
          `${frame.command ? echo(frame.command) : ""}${fg(palette.colors.danger)}✕ ${frame.message}${RESET}\r\n`,
        );
        break;
    }
  });

  useEffect(() => {
    const el = host.current;
    if (!el) return;
    const t = new Terminal({
      fontFamily:
        getComputedStyle(document.documentElement).getPropertyValue("--font-mono").trim() || "monospace",
      fontSize: 11,
      lineHeight: 1.9,
      scrollback: 10_000,
      disableStdin: true,
      cursorBlink: false,
      cursorInactiveStyle: "none",
      convertEol: false,
      allowProposedApi: true,
      minimumContrastRatio: 4.5,
      theme: firstPalette.current.theme,
    });
    const fit = new FitAddon();
    const search = new SearchAddon();
    t.loadAddon(fit);
    t.loadAddon(search);
    t.open(el);
    t.write(HIDE_CURSOR);
    term.current = t;
    searcher.current = search;
    const results = search.onDidChangeResults(({ resultIndex, resultCount }) =>
      setResults({ index: resultIndex, count: resultCount }),
    );
    const scroll = t.onScroll(() => {
      const buffer = t.buffer.active;
      const atBottom = buffer.viewportY >= buffer.baseY;
      followingRef.current = atBottom;
      setFollowing(atBottom);
      if (atBottom) setUnseen(0);
    });
    t.attachCustomKeyEventHandler((event) => {
      if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "f" && event.type === "keydown") {
        setSearchOpen(true);
        return false;
      }
      return true;
    });
    let frame = 0;
    const observer = new ResizeObserver(() => {
      cancelAnimationFrame(frame);
      frame = requestAnimationFrame(() => fit.fit());
    });
    observer.observe(el);
    fit.fit();
    return () => {
      observer.disconnect();
      cancelAnimationFrame(frame);
      results.dispose();
      scroll.dispose();
      t.dispose();
      term.current = null;
      searcher.current = null;
    };
  }, []);

  // A theme change recolors the terminal in place; lines already printed
  // keep their colors, which the contrast floor keeps readable.
  useEffect(() => {
    if (term.current) term.current.options.theme = palette.theme;
  }, [palette]);

  const decorations = {
    matchBackground: palette.search.match,
    activeMatchBackground: palette.search.active,
    matchOverviewRuler: palette.search.active,
    activeMatchColorOverviewRuler: palette.search.active,
  };
  const find = (direction: "next" | "previous", text = query) => {
    const search = searcher.current;
    if (!search) return;
    if (!text) {
      search.clearDecorations();
      setResults(null);
      return;
    }
    const options = { decorations, incremental: direction === "next" };
    if (direction === "next") search.findNext(text, options);
    else search.findPrevious(text, options);
  };

  const copy = async () => {
    const t = term.current;
    if (!t) return;
    let text = t.getSelection();
    if (!text) {
      const buffer = t.buffer.active;
      const lines: string[] = [];
      for (let i = 0; i < buffer.length; i++) lines.push(buffer.getLine(i)?.translateToString(true) ?? "");
      text = lines.join("\n").trimEnd();
    }
    try {
      await navigator.clipboard.writeText(text);
      toast.success(t.hasSelection() ? "Copied the selection." : "Copied the console output.");
    } catch {
      toast.error("Couldn't copy. Select the text and copy it with your keyboard instead.");
    }
  };

  const running = server.status === ServerStatus.RUNNING;
  const live = state === "live";

  return (
    <section
      aria-label="Server console"
      className="overflow-hidden rounded-[9px] border border-border bg-console"
    >
      <div className="flex flex-wrap items-center gap-3 border-border border-b bg-surface px-[17px] py-[11px] text-meta">
        <SquareTerminal className="size-4" aria-hidden />
        Server console
        <span
          role="status"
          className={cn("inline-flex items-center gap-1.5", live ? "text-success" : "text-text-muted")}
        >
          <span aria-hidden className={cn("size-1.5 rounded-full bg-current", !live && "animate-pulse")} />
          {STATE_LABEL[state]}
        </span>
        <span className="ml-auto flex items-center gap-1">
          <Button
            size="icon"
            variant="ghost"
            className="size-7"
            aria-label="Search the console"
            aria-pressed={searchOpen}
            onClick={() => setSearchOpen((open) => !open)}
          >
            <Search className="size-[15px]" />
          </Button>
          <Button
            size="icon"
            variant="ghost"
            className="size-7"
            aria-label="Copy console output"
            onClick={copy}
          >
            <ClipboardCopy className="size-[15px]" />
          </Button>
          <Button
            size="icon"
            variant="ghost"
            className="size-7"
            aria-label="Clear the console view"
            onClick={() => {
              term.current?.clear();
              setUnseen(0);
            }}
          >
            <Eraser className="size-[15px]" />
          </Button>
          <span className="ml-1.5">
            <ServerStatusBadge
              status={server.status}
              nodeOffline={server.nodeOffline}
              nodeUpdateNeeded={server.nodeUpdateNeeded}
            />
          </span>
        </span>
      </div>

      {searchOpen && (
        <div className="flex items-center gap-2 border-border border-b bg-surface-subtle px-[17px] py-2">
          <Search className="size-[14px] text-text-muted" aria-hidden />
          <input
            // biome-ignore lint/a11y/noAutofocus: opening search is an explicit request to type
            autoFocus
            aria-label="Search the console"
            placeholder="Search output…"
            value={query}
            onChange={(e) => {
              setQuery(e.target.value);
              find("next", e.target.value);
            }}
            onKeyDown={(e) => {
              if (e.key === "Enter") find(e.shiftKey ? "previous" : "next");
              if (e.key === "Escape") {
                setSearchOpen(false);
                searcher.current?.clearDecorations();
                term.current?.focus();
              }
            }}
            className="min-w-0 flex-1 bg-transparent font-mono text-meta text-text-primary outline-none placeholder:text-text-muted"
          />
          <span className="text-meta text-text-muted tabular-nums" aria-live="polite">
            {query && results
              ? results.count === 0
                ? "No matches"
                : `${results.index + 1} of ${results.count}`
              : ""}
          </span>
          <Button
            size="icon"
            variant="ghost"
            className="size-6"
            aria-label="Previous match"
            onClick={() => find("previous")}
          >
            <ArrowUp className="size-[13px]" />
          </Button>
          <Button
            size="icon"
            variant="ghost"
            className="size-6"
            aria-label="Next match"
            onClick={() => find("next")}
          >
            <ArrowDown className="size-[13px]" />
          </Button>
          <Button
            size="icon"
            variant="ghost"
            className="size-6"
            aria-label="Close search"
            onClick={() => {
              setSearchOpen(false);
              searcher.current?.clearDecorations();
            }}
          >
            <X className="size-[13px]" />
          </Button>
        </div>
      )}

      <div className="relative">
        <div ref={host} className="h-[360px] px-5 py-3 below-md:h-[320px] below-sm:px-3" />
        {!hasOutput && (
          <p className="pointer-events-none absolute inset-0 grid place-items-center p-8 text-center text-label text-text-muted">
            {state !== "live"
              ? "Connecting to the console…"
              : running || server.status === ServerStatus.STARTING
                ? "Waiting for output…"
                : "No output yet. Start the server to see its console here."}
          </p>
        )}
        {!following && (
          <Button
            size="mini"
            variant="accent"
            className="absolute right-5 bottom-3 shadow-overlay"
            onClick={() => term.current?.scrollToBottom()}
          >
            <ArrowDown />
            {unseen > 0 ? "New output" : "Jump to latest"}
          </Button>
        )}
      </div>

      <CommandInput
        serverId={server.id}
        enabled={live && running}
        reason={!live ? "Connecting to the console…" : "Commands can be sent once the server is running."}
        onSend={(command) => {
          if (!send(command)) return false;
          term.current?.scrollToBottom();
          return true;
        }}
      />
    </section>
  );
}

function CommandInput({
  serverId,
  enabled,
  reason,
  onSend,
}: {
  serverId: string;
  enabled: boolean;
  reason: string;
  onSend: (command: string) => boolean;
}) {
  const history = useHistory(serverId);
  const [value, setValue] = useState("");
  // Position while browsing history: entries.length means "the draft".
  const [cursor, setCursor] = useState<number | null>(null);
  const [draft, setDraft] = useState("");
  const suggestions = completions(value);

  const submit = () => {
    const command = value.trim();
    if (!command || !enabled) return;
    if (onSend(command)) {
      history.push(command);
      setValue("");
      setCursor(null);
    }
  };

  const browse = (step: -1 | 1) => {
    const entries = history.entries;
    if (entries.length === 0) return;
    const current = cursor ?? entries.length;
    const next = Math.max(0, Math.min(entries.length, current + step));
    if (cursor === null) setDraft(value);
    setCursor(next === entries.length ? null : next);
    setValue(next === entries.length ? draft : (entries[next] ?? ""));
  };

  return (
    <form
      className="relative border-border border-t"
      onSubmit={(e) => {
        e.preventDefault();
        submit();
      }}
    >
      {enabled && suggestions.length > 0 && (
        <div className="absolute inset-x-0 bottom-full flex flex-wrap items-center gap-1.5 border-border border-t bg-surface-subtle px-4 py-2 text-meta">
          <span className="text-text-muted">Tab to complete:</span>
          {suggestions.map((s) => (
            <button
              key={s}
              type="button"
              className="rounded-xs bg-surface-elevated px-1.5 py-0.5 font-mono text-text-secondary hover:text-text-primary"
              onClick={() => setValue(`${s} `)}
            >
              {s}
            </button>
          ))}
        </div>
      )}
      <div className="flex items-center gap-[9px] px-4 py-3">
        <span aria-hidden className="font-mono text-primary">
          ›
        </span>
        <input
          aria-label="Console command"
          autoComplete="off"
          spellCheck={false}
          disabled={!enabled}
          placeholder={enabled ? "Enter a command (try list or help)…" : reason}
          value={value}
          onChange={(e) => {
            setValue(e.target.value);
            setCursor(null);
          }}
          onKeyDown={(e) => {
            if (e.key === "ArrowUp") {
              e.preventDefault();
              browse(-1);
            } else if (e.key === "ArrowDown") {
              e.preventDefault();
              browse(1);
            } else if (e.key === "Tab" && suggestions[0]) {
              e.preventDefault();
              setValue(`${suggestions[0]} `);
            }
          }}
          className="min-w-0 flex-1 bg-transparent font-mono text-meta text-text-primary outline-none placeholder:text-text-muted disabled:cursor-not-allowed"
        />
        <Button type="submit" size="mini" disabled={!enabled || !value.trim()}>
          Send
          <ArrowRight />
        </Button>
      </div>
    </form>
  );
}
