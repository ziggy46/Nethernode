/**
 * Fails if application source contains raw hex colors. Colors belong in
 * src/styles/tokens.css; components use semantic utilities or CSS variables.
 */
import { readdirSync, readFileSync, statSync } from "node:fs";
import { join, relative } from "node:path";

const root = new URL("../src", import.meta.url).pathname;
const SKIP = new Set(["gen"]);
const HEX = /#[0-9a-fA-F]{6}\b|#[0-9a-fA-F]{3}\b(?![\w-])/g;

function* files(dir: string): Generator<string> {
  for (const name of readdirSync(dir)) {
    const path = join(dir, name);
    if (statSync(path).isDirectory()) {
      if (!SKIP.has(name)) yield* files(path);
    } else if (/\.(tsx?|jsx?)$/.test(name)) {
      yield path;
    }
  }
}

const hits: string[] = [];
for (const file of files(root)) {
  readFileSync(file, "utf8")
    .split("\n")
    .forEach((line, i) => {
      for (const match of line.matchAll(HEX)) {
        hits.push(`${relative(root, file)}:${i + 1}: ${match[0]}`);
      }
    });
}

if (hits.length > 0) {
  console.error(`Raw hex colors found outside tokens.css:\n${hits.join("\n")}`);
  process.exit(1);
}
console.log("No raw hex colors in components.");
