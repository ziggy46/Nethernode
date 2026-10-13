/**
 * Design tokens for code that can't use Tailwind classes (uPlot charts,
 * xterm.js). Values are read from the CSS custom properties defined in
 * tokens.css at runtime, so the hex values live in exactly one place.
 */

const TOKEN_NAMES = [
  "background",
  "sidebar",
  "surface-subtle",
  "surface",
  "surface-elevated",
  "console",
  "border",
  "primary",
  "lava",
  "crimson",
  "text-primary",
  "text-secondary",
  "text-muted",
  "success",
  "warning",
  "danger",
  "info",
  "meter-memory",
  "meter-storage",
] as const;

export type TokenName = (typeof TOKEN_NAMES)[number];
export type Tokens = Record<TokenName, string>;

/** Reads every color token from the document root. */
export function readTokens(root: Element = document.documentElement): Tokens {
  const style = getComputedStyle(root);
  const entries = TOKEN_NAMES.map((name) => [name, style.getPropertyValue(`--color-${name}`).trim()]);
  return Object.fromEntries(entries) as Tokens;
}

/** Reads one color token. */
export function token(name: TokenName, root: Element = document.documentElement): string {
  return getComputedStyle(root).getPropertyValue(`--color-${name}`).trim();
}
