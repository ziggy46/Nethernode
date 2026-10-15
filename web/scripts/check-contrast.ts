/**
 * Fails if any text pair in tokens.css drops below 4.5:1 or any interactive
 * boundary below 3:1 (WCAG 2.1 AA), in the dark and the light theme. Run
 * with `pnpm check:contrast`.
 */
import { readFileSync } from "node:fs";
import { evaluate, parseThemes } from "./contrast-lib.ts";

const css = readFileSync(new URL("../src/styles/tokens.css", import.meta.url), "utf8");
const themes = parseThemes(css);
const results = [
  ...evaluate(themes.dark).map((r) => ({ ...r, theme: "dark" })),
  ...evaluate(themes.light).map((r) => ({ ...r, theme: "light" })),
];
const failures = results.filter((r) => !r.pass);
const verbose = process.argv.includes("--verbose");

for (const r of results) {
  if (!verbose && r.pass) continue;
  const rule = r.min === null ? "decorative" : `${r.kind} ≥ ${r.min}`;
  const mark = r.pass ? "ok  " : "FAIL";
  console.log(
    `${mark} ${r.theme.padEnd(5)} ${r.ratio.toFixed(2).padStart(5)}  ${r.fg} on ${r.bg}  (${rule})`,
  );
}

const enforced = results.filter((r) => r.min !== null).length;
if (failures.length > 0) {
  console.error(`\n${failures.length} of ${enforced} contrast checks failed.`);
  process.exit(1);
}
console.log(`All ${enforced} contrast checks pass.`);
