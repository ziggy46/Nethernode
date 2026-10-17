/**
 * Text helpers for the console: Minecraft `§` formatting codes to ANSI,
 * coloring for plain log lines, and command completion.
 */

/** An sRGB color as 0–255 channels. */
export type Rgb = readonly [number, number, number];

/** Console colors resolved from the design tokens. */
export interface ConsoleColors {
  dim: Rgb;
  success: Rgb;
  warning: Rgb;
  danger: Rgb;
  prompt: Rgb;
}

const ESC = "\x1b[";
const RESET = `${ESC}0m`;

export function fg([r, g, b]: Rgb): string {
  return `${ESC}38;2;${r};${g};${b}m`;
}

/** `§` color codes mapped onto the 16-color ANSI palette (themed by tokens). */
const SECTION_COLORS: Record<string, string> = {
  "0": "30",
  "1": "34",
  "2": "32",
  "3": "36",
  "4": "31",
  "5": "35",
  "6": "33",
  "7": "37",
  "8": "90",
  "9": "94",
  a: "92",
  b: "96",
  c: "91",
  d: "95",
  e: "93",
  f: "97",
};
const SECTION_STYLES: Record<string, string> = { l: "1", m: "9", n: "4", o: "3", r: "0" };

/**
 * Converts Minecraft's `§` codes (as in RCON replies) to ANSI escapes,
 * including `§x§R§R§G§G§B§B` hex colors. Obfuscated text (`§k`) is shown
 * plainly.
 */
export function sectionToAnsi(text: string): string {
  if (!text.includes("§")) return text;
  let out = "";
  let styled = false;
  for (let i = 0; i < text.length; i++) {
    const ch = text[i];
    if (ch !== "§" || i + 1 >= text.length) {
      out += ch;
      continue;
    }
    const code = (text[i + 1] ?? "").toLowerCase();
    if (code === "x") {
      const hex = text.slice(i + 2, i + 14).replaceAll("§", "");
      if (/^[0-9a-f]{6}$/i.test(hex)) {
        const n = Number.parseInt(hex, 16);
        out += fg([(n >> 16) & 255, (n >> 8) & 255, n & 255]);
        styled = true;
        i += 13;
        continue;
      }
    }
    const color = SECTION_COLORS[code];
    const style = SECTION_STYLES[code];
    if (color) {
      // A color code also resets formatting, as in Minecraft.
      out += `${RESET}${ESC}${color}m`;
      styled = true;
    } else if (style) {
      out += `${ESC}${style}m`;
      styled = style !== "0";
    }
    i += 1;
  }
  return styled ? out + RESET : out;
}

const TIMESTAMP = /^(\[\d{2}:\d{2}:\d{2}[^\]]*\]|\d{4}-\d{2}-\d{2}T\S+)(.*)$/;

/**
 * Colors a plain log line the way the UI reference does: dim timestamp,
 * amber warnings, red errors, green "Done". Lines that already carry ANSI
 * colors (Paper prints its own) are left alone.
 */
export function colorize(line: string, colors: ConsoleColors): string {
  if (line.includes(ESC)) return line;
  const level = /\b(?:WARN|WARNING)\b/.test(line)
    ? colors.warning
    : /\b(?:ERROR|SEVERE|FATAL)\b|Exception\b/.test(line)
      ? colors.danger
      : /\bDone \(|For help, type/.test(line)
        ? colors.success
        : null;
  const match = TIMESTAMP.exec(line);
  const [stamp, rest] = match ? [match[1] ?? "", match[2] ?? ""] : ["", line];
  const head = stamp ? `${fg(colors.dim)}${stamp}${RESET}` : "";
  return level ? `${head}${fg(level)}${rest}${RESET}` : `${head}${rest}`;
}

/** Vanilla commands for completion (Java Edition 1.21). */
export const VANILLA_COMMANDS = [
  "advancement",
  "attribute",
  "ban",
  "ban-ip",
  "banlist",
  "bossbar",
  "clear",
  "clone",
  "damage",
  "data",
  "datapack",
  "debug",
  "defaultgamemode",
  "deop",
  "difficulty",
  "effect",
  "enchant",
  "execute",
  "experience",
  "fill",
  "fillbiome",
  "forceload",
  "function",
  "gamemode",
  "gamerule",
  "give",
  "help",
  "item",
  "jfr",
  "kick",
  "kill",
  "list",
  "locate",
  "loot",
  "me",
  "msg",
  "op",
  "pardon",
  "pardon-ip",
  "particle",
  "perf",
  "place",
  "playsound",
  "random",
  "recipe",
  "reload",
  "return",
  "ride",
  "rotate",
  "save-all",
  "save-off",
  "save-on",
  "say",
  "schedule",
  "scoreboard",
  "seed",
  "setblock",
  "setidletimeout",
  "setworldspawn",
  "spawnpoint",
  "spectate",
  "spreadplayers",
  "stop",
  "stopsound",
  "summon",
  "tag",
  "team",
  "teammsg",
  "teleport",
  "tell",
  "tellraw",
  "tick",
  "time",
  "title",
  "tm",
  "tp",
  "transfer",
  "trigger",
  "w",
  "weather",
  "whitelist",
  "worldborder",
  "xp",
] as const;

/** Player names to complete, online ones first. */
export interface KnownPlayers {
  online: string[];
  offline: string[];
}

/** A suggestion: what's shown, and the input once it's chosen. */
export interface Completion {
  label: string;
  value: string;
  /** Shown faintly beside the label. */
  hint?: string;
}

const SELECTORS = ["@a", "@p", "@r", "@s", "@e"];
const SELECTOR_HINTS: Record<string, string> = {
  "@a": "all players",
  "@p": "nearest player",
  "@r": "a random player",
  "@s": "the one running it",
  "@e": "all entities",
};

const GAME_MODES = ["survival", "creative", "adventure", "spectator"];

/**
 * What each command takes, per argument: a list of words, or `player`.
 * Arguments not listed aren't completed.
 */
const ARGUMENTS: Record<string, (readonly string[] | "player")[]> = {
  ban: ["player"],
  clear: ["player"],
  defaultgamemode: [GAME_MODES],
  deop: ["player"],
  difficulty: [["peaceful", "easy", "normal", "hard"]],
  effect: [["give", "clear"], "player"],
  enchant: ["player"],
  experience: [["add", "set", "query"], "player"],
  gamemode: [GAME_MODES, "player"],
  give: ["player"],
  kick: ["player"],
  kill: ["player"],
  msg: ["player"],
  op: ["player"],
  pardon: ["player"],
  "save-all": [["flush"]],
  spawnpoint: ["player"],
  spectate: ["player"],
  teleport: ["player", "player"],
  tell: ["player"],
  tellraw: ["player"],
  time: [["set", "add", "query"]],
  title: ["player", ["title", "subtitle", "actionbar", "clear", "reset", "times"]],
  tp: ["player", "player"],
  w: ["player"],
  weather: [["clear", "rain", "thunder"]],
  whitelist: [["add", "remove", "on", "off", "list", "reload"], "player"],
  xp: [["add", "set", "query"], "player"],
};

/**
 * Suggestions for the word being typed in `input`: command names first,
 * then the command's arguments (players, online ones first, and fixed
 * words like game modes). Choosing one replaces that word.
 */
export function completions(input: string, players?: KnownPlayers, limit = 8): Completion[] {
  const slash = input.startsWith("/") ? "/" : "";
  const words = input.slice(slash.length).split(" ");
  const typing = words[words.length - 1] ?? "";
  const word = typing.toLowerCase();
  const before = `${slash}${words.slice(0, -1).join(" ")}${words.length > 1 ? " " : ""}`;
  const pick = (options: Completion[]) =>
    options
      .filter((o) => o.label.toLowerCase().startsWith(word) && o.label.toLowerCase() !== word)
      .slice(0, limit);

  if (words.length === 1) {
    if (!word) return [];
    return pick(VANILLA_COMMANDS.map((c) => ({ label: c, value: `${slash}${c} ` })));
  }
  const command = (words[0] ?? "").toLowerCase();
  const kind = ARGUMENTS[command]?.[words.length - 2];
  if (!kind) return [];
  const choose = (label: string, hint?: string): Completion => ({ label, value: `${before}${label} `, hint });
  if (kind !== "player") return pick(kind.map((w) => choose(w)));
  const online = players?.online ?? [];
  const offline = (players?.offline ?? []).filter((p) => !online.includes(p));
  const options = [
    ...online.map((p) => choose(p, "online")),
    ...(word.startsWith("@") ? SELECTORS.map((s) => choose(s, SELECTOR_HINTS[s])) : []),
    ...offline.map((p) => choose(p)),
  ];
  // Offline players only once something's typed: the list could be long.
  return pick(word ? options : options.filter((o) => o.hint === "online"));
}
