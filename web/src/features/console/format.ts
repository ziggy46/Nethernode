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

/**
 * Commands that complete the first word of `input`, or none once the
 * command name is typed out (arguments aren't completed).
 */
export function completions(input: string, limit = 6): string[] {
  const text = input.replace(/^\//, "");
  if (text.includes(" ")) return [];
  const word = text.toLowerCase();
  if (!word) return [];
  return VANILLA_COMMANDS.filter((c) => c.startsWith(word) && c !== word).slice(0, limit);
}
