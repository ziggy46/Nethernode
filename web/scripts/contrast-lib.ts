/**
 * Color parsing and WCAG 2.1 contrast math for the token checker.
 * Supports the two forms tokens.css uses: `#rrggbb` and
 * `color-mix(in oklab, <a> <p>%, <b>)` where a and b are hex or `var(--*)`.
 */

export type Rgb = readonly [number, number, number];

/**
 * Extracts every custom property declaration, keyed by name without the
 * leading `--` (e.g. `color-primary`, `input`). Comments are ignored.
 */
export function parseColorTokens(css: string): Map<string, string> {
  const tokens = new Map<string, string>();
  const code = css.replace(/\/\*[\s\S]*?\*\//g, "");
  for (const [, name, value] of code.matchAll(/--([\w-]+)\s*:\s*([^;{}]+);/g)) {
    if (name && value && !tokens.has(name)) tokens.set(name, value.trim());
  }
  return tokens;
}

/** Resolves a token value to sRGB (0–255), following var() and color-mix(). */
export function resolveColor(value: string, tokens: Map<string, string>, depth = 0): Rgb | null {
  if (depth > 10) throw new Error(`token reference cycle at ${value}`);
  const v = value.trim();
  const hex = /^#([0-9a-f]{6})$/i.exec(v);
  if (hex?.[1]) {
    const n = Number.parseInt(hex[1], 16);
    return [(n >> 16) & 255, (n >> 8) & 255, n & 255];
  }
  const ref = /^var\(--([\w-]+)\)$/.exec(v);
  if (ref?.[1]) {
    const target = tokens.get(ref[1]);
    return target ? resolveColor(target, tokens, depth + 1) : null;
  }
  const mix = /^color-mix\(in oklab,\s*(.+?)\s+(\d+(?:\.\d+)?)%,\s*(.+)\)$/.exec(v);
  if (mix?.[1] && mix[2] && mix[3]) {
    const a = resolveColor(mix[1], tokens, depth + 1);
    const b = resolveColor(mix[3], tokens, depth + 1);
    if (!a || !b) return null;
    return mixOklab(a, b, Number(mix[2]) / 100);
  }
  // var(--x) pointing at non-color variables, rgb() with alpha, etc.
  return null;
}

function srgbToLinear(c: number): number {
  const s = c / 255;
  return s <= 0.04045 ? s / 12.92 : ((s + 0.055) / 1.055) ** 2.4;
}

function linearToSrgb(c: number): number {
  const s = c <= 0.0031308 ? 12.92 * c : 1.055 * c ** (1 / 2.4) - 0.055;
  return Math.round(Math.min(1, Math.max(0, s)) * 255);
}

function toOklab([r, g, b]: Rgb): [number, number, number] {
  const [lr, lg, lb] = [srgbToLinear(r), srgbToLinear(g), srgbToLinear(b)];
  const l = Math.cbrt(0.4122214708 * lr + 0.5363325363 * lg + 0.0514459929 * lb);
  const m = Math.cbrt(0.2119034982 * lr + 0.6806995451 * lg + 0.1073969566 * lb);
  const s = Math.cbrt(0.0883024619 * lr + 0.2817188376 * lg + 0.6299787005 * lb);
  return [
    0.2104542553 * l + 0.793617785 * m - 0.0040720468 * s,
    1.9779984951 * l - 2.428592205 * m + 0.4505937099 * s,
    0.0259040371 * l + 0.7827717662 * m - 0.808675766 * s,
  ];
}

function fromOklab([L, a, b]: [number, number, number]): Rgb {
  const l = (L + 0.3963377774 * a + 0.2158037573 * b) ** 3;
  const m = (L - 0.1055613458 * a - 0.0638541728 * b) ** 3;
  const s = (L - 0.0894841775 * a - 1.291485548 * b) ** 3;
  return [
    linearToSrgb(4.0767416621 * l - 3.3077115913 * m + 0.2309699292 * s),
    linearToSrgb(-1.2684380046 * l + 2.6097574011 * m - 0.3413193965 * s),
    linearToSrgb(-0.0041960863 * l - 0.7034186147 * m + 1.707614701 * s),
  ];
}

/** `color-mix(in oklab, a p, b)`: p is the weight of a (0–1). */
export function mixOklab(a: Rgb, b: Rgb, p: number): Rgb {
  const A = toOklab(a);
  const B = toOklab(b);
  const lerp = (x: number, y: number) => x * p + y * (1 - p);
  return fromOklab([lerp(A[0], B[0]), lerp(A[1], B[1]), lerp(A[2], B[2])]);
}

export function relativeLuminance([r, g, b]: Rgb): number {
  return 0.2126 * srgbToLinear(r) + 0.7152 * srgbToLinear(g) + 0.0722 * srgbToLinear(b);
}

/** WCAG 2.1 contrast ratio, 1–21. */
export function contrastRatio(fg: Rgb, bg: Rgb): number {
  const a = relativeLuminance(fg);
  const b = relativeLuminance(bg);
  return (Math.max(a, b) + 0.05) / (Math.min(a, b) + 0.05);
}

export type PairKind = "text" | "boundary" | "decorative";

export interface Pair {
  fg: string;
  bg: string;
  kind: PairKind;
}

export const TEXT_MIN = 4.5;
export const BOUNDARY_MIN = 3;

const SURFACES = ["background", "sidebar", "surface-subtle", "surface", "surface-elevated"];

/** Every foreground/background pairing the UI uses, with the rule it must meet. */
export function requiredPairs(): Pair[] {
  const pairs: Pair[] = [];
  const add = (fgs: string[], bgs: string[], kind: PairKind) => {
    for (const fg of fgs) for (const bg of bgs) pairs.push({ fg, bg, kind });
  };
  const textColors = [
    "text-primary",
    "text-secondary",
    "text-muted",
    "primary",
    "lava",
    "success",
    "warning",
    "danger",
    "info",
  ];
  add(textColors, SURFACES, "text");
  add(["text-primary", "text-secondary", "text-muted"], ["field", "console", "button-quiet"], "text");
  add(["text-primary", "text-secondary"], ["nav-hover", "button-quiet-hover", "nav-active"], "text");

  // Filled buttons and chips carry dark text.
  add(
    ["primary-foreground"],
    ["primary", "primary-hover", "primary-pressed", "danger", "success", "warning"],
    "text",
  );
  add(["nav-active-foreground"], ["nav-active", "sidebar"], "text");
  add(["danger"], ["button-danger"], "text");
  add(["primary"], ["primary-subtle"], "text");

  // Badges: status-colored text on its own subtle background.
  for (const status of ["success", "warning", "danger", "info"]) {
    add([status], [`${status}-subtle`], "text");
  }
  // Muted status label ("Offline") on a subtle surface.
  add(["text-secondary"], ["surface-elevated"], "text");

  // Interactive boundaries (WCAG 1.4.11): control outlines and focus rings.
  add(["border-strong", "primary"], [...SURFACES, "field"], "boundary");
  add(["border-strong"], ["button-quiet"], "boundary");

  // Decorative only; reported, never enforced.
  add(["border", "crimson", "meter-memory", "meter-storage"], SURFACES, "decorative");
  return pairs;
}

export interface Result extends Pair {
  ratio: number;
  min: number | null;
  pass: boolean;
}

export function evaluate(tokens: Map<string, string>): Result[] {
  return requiredPairs().map((pair) => {
    const fgValue = tokens.get(`color-${pair.fg}`);
    const bgValue = tokens.get(`color-${pair.bg}`);
    if (!fgValue || !bgValue) throw new Error(`missing token: ${!fgValue ? pair.fg : pair.bg}`);
    const fg = resolveColor(fgValue, tokens);
    const bg = resolveColor(bgValue, tokens);
    if (!fg || !bg) throw new Error(`unresolvable token in ${pair.fg} on ${pair.bg}`);
    const ratio = contrastRatio(fg, bg);
    const min = pair.kind === "text" ? TEXT_MIN : pair.kind === "boundary" ? BOUNDARY_MIN : null;
    return { ...pair, ratio, min, pass: min === null || ratio >= min };
  });
}
