// @vitest-environment node
import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { contrastRatio, evaluate, mixOklab, parseColorTokens, resolveColor } from "./contrast-lib.ts";

const css = readFileSync(new URL("../src/styles/tokens.css", import.meta.url), "utf8");
const tokens = parseColorTokens(css);
const ratio = (fg: string, bg: string) => {
  const a = resolveColor(`var(--color-${fg})`, tokens);
  const b = resolveColor(`var(--color-${bg})`, tokens);
  if (!a || !b) throw new Error("unresolved");
  return contrastRatio(a, b);
};

describe("contrast math", () => {
  it("matches the verified table in 06-design-system.md", () => {
    const table: [string, string, number][] = [
      ["text-primary", "background", 17.29],
      ["text-muted", "surface-elevated", 4.51],
      ["primary", "surface", 7.28],
      ["crimson", "surface", 3.0],
      ["border", "surface", 1.39],
      ["border-strong", "surface-elevated", 3.08],
      ["primary-foreground", "primary", 8.17],
      ["primary-foreground", "danger", 6.77],
    ];
    for (const [fg, bg, expected] of table) {
      expect(ratio(fg, bg), `${fg} on ${bg}`).toBeCloseTo(expected, 1);
    }
  });

  it("black on white is 21:1", () => {
    expect(contrastRatio([0, 0, 0], [255, 255, 255])).toBeCloseTo(21, 5);
  });

  it("oklab mixing hits its endpoints", () => {
    expect(mixOklab([255, 0, 0], [0, 0, 255], 1)).toEqual([255, 0, 0]);
    expect(mixOklab([255, 0, 0], [0, 0, 255], 0)).toEqual([0, 0, 255]);
  });

  it("resolves derived tokens", () => {
    expect(resolveColor("var(--color-success-subtle)", tokens)).not.toBeNull();
    expect(resolveColor("var(--color-input)", tokens)).toEqual(resolveColor("#7a6890", tokens));
  });

  it("every enforced pair passes", () => {
    expect(evaluate(tokens).filter((r) => !r.pass)).toEqual([]);
  });
});
