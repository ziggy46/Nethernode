/**
 * Fails if the JavaScript loaded for the signed-in app shell exceeds the
 * 200 KB gzip budget from 08-performance-budget.md: the entry script, its
 * modulepreloads, the authenticated layout's chunks (`_app-*.js`), and
 * everything those import statically. Lazy `import()`s are excluded.
 * Run after `vite build`.
 */
import { readdirSync, readFileSync } from "node:fs";
import { gzipSync } from "node:zlib";

const BUDGET = 200 * 1024;
const dist = new URL("../dist/", import.meta.url);
const html = readFileSync(new URL("index.html", dist), "utf8");
const scripts = [
  ...html.matchAll(/<script[^>]+src="\/([^"]+\.js)"/g),
  ...html.matchAll(/<link[^>]+rel="modulepreload"[^>]+href="\/([^"]+\.js)"/g),
].flatMap((m) => (m[1] ? [m[1]] : []));
const layoutChunks = readdirSync(new URL("assets/", dist))
  .filter((name) => /^_app-.*\.js$/.test(name))
  .map((name) => `assets/${name}`);
scripts.push(...layoutChunks);

// Follow static imports (`from"./x.js"`, `import"./x.js"`), not `import("./x.js")`.
const seen = new Set<string>();
const queue = [...scripts];
while (queue.length > 0) {
  const path = queue.pop();
  if (!path || seen.has(path)) continue;
  seen.add(path);
  const code = readFileSync(new URL(path, dist), "utf8");
  for (const match of code.matchAll(/(?:\bfrom\s*|\bimport\s*)"\.\/([^"]+\.js)"/g)) {
    if (match[1]) queue.push(`assets/${match[1]}`);
  }
}
scripts.splice(0, scripts.length, ...seen);

let total = 0;
for (const path of new Set(scripts)) {
  const size = gzipSync(readFileSync(new URL(path, dist))).length;
  total += size;
  console.log(`${(size / 1024).toFixed(1).padStart(7)} KB  ${path}`);
}
console.log(`${(total / 1024).toFixed(1).padStart(7)} KB  shell total (gzip), budget ${BUDGET / 1024} KB`);
if (total > BUDGET) {
  console.error("Shell JavaScript exceeds the budget.");
  process.exit(1);
}
