/** Path helpers for the file manager. Paths are relative, `/`-separated, and "" is the server folder. */

export function joinPath(dir: string, name: string): string {
  return dir ? `${dir}/${name}` : name;
}

export function parentPath(path: string): string {
  const i = path.lastIndexOf("/");
  return i < 0 ? "" : path.slice(0, i);
}

export function baseName(path: string): string {
  return path.slice(path.lastIndexOf("/") + 1);
}

/** Breadcrumb segments: `[{ name, path }]` from the root down. */
export function crumbs(path: string): { name: string; path: string }[] {
  if (!path) return [];
  const parts = path.split("/");
  return parts.map((name, i) => ({ name, path: parts.slice(0, i + 1).join("/") }));
}

/** Whether `name` is a single valid file or folder name. */
export function validName(name: string): string | null {
  const trimmed = name.trim();
  if (!trimmed) return "Enter a name.";
  if (trimmed === "." || trimmed === "..") return "That name isn't allowed.";
  if (/[/\\\0]/.test(trimmed)) return "Names can't contain / or \\.";
  if (trimmed.length > 255) return "That name is too long.";
  return null;
}

export type EditorLanguage = "json" | "yaml" | "properties" | "toml" | "plain";

/** Syntax highlighting by extension. */
export function languageFor(path: string): EditorLanguage {
  const name = baseName(path).toLowerCase();
  const ext = name.includes(".") ? name.slice(name.lastIndexOf(".") + 1) : "";
  if (["json", "json5", "mcmeta", "jsonc"].includes(ext)) return "json";
  if (["yml", "yaml"].includes(ext)) return "yaml";
  if (["properties", "lang"].includes(ext)) return "properties";
  if (ext === "toml") return "toml";
  return "plain";
}

export function isZip(path: string): boolean {
  return path.toLowerCase().endsWith(".zip");
}

/**
 * Files a running server keeps in memory and writes back on its own, so
 * edits made while it runs are likely to be overwritten.
 */
const SERVER_OWNED = new Set([
  "ops.json",
  "whitelist.json",
  "banned-players.json",
  "banned-ips.json",
  "usercache.json",
]);

/** A warning for editing `path` while the server runs, if one applies. */
export function runningEditWarning(path: string): string | null {
  if (SERVER_OWNED.has(path)) {
    return "The server keeps this file in memory and rewrites it while running, so edits made now can be lost. Use console commands (op, whitelist, ban) instead, or stop the server first.";
  }
  if (path === "server.properties") {
    return "The server reads this file when it starts, so changes apply after a restart. Some values are set by NetherNode on every start; the Configuration tab shows which.";
  }
  return null;
}

/** `1.2 MB`-style sizes. */
export function formatSize(bytes: number | bigint): string {
  const n = Number(bytes);
  if (n < 1024) return `${n} B`;
  const units = ["KB", "MB", "GB", "TB"];
  let value = n / 1024;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit++;
  }
  return `${value < 10 ? value.toFixed(1) : Math.round(value)} ${units[unit]}`;
}

export { ago } from "@/lib/time";
