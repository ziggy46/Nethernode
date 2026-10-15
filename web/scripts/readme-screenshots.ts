/**
 * Captures the README screenshots from a running panel with some servers.
 *
 *   NN_URL=http://127.0.0.1:8080 NN_USER=admin NN_PASSWORD=… NN_SERVER=<server id> \
 *     node scripts/readme-screenshots.ts
 *
 * Writes PNGs to ../assets/screenshots/.
 */
import { chromium } from "@playwright/test";

const url = process.env.NN_URL ?? "http://127.0.0.1:8080";
const user = process.env.NN_USER;
const password = process.env.NN_PASSWORD;
const server = process.env.NN_SERVER;
if (!user || !password || !server) {
  console.error("Set NN_USER, NN_PASSWORD and NN_SERVER (a running server's ID).");
  process.exit(1);
}
const out = new URL("../../assets/screenshots/", import.meta.url).pathname;

const browser = await chromium.launch();
const page = await browser.newPage({
  viewport: { width: 1440, height: 900 },
  deviceScaleFactor: 2,
  colorScheme: "dark",
});
await page.goto(`${url}/login`);
await page.getByLabel("Username").fill(user);
await page.getByLabel("Password", { exact: true }).fill(password);
await page.getByRole("button", { name: "Sign in" }).click();
await page.waitForURL(`${url}/`);

const shots: [string, string, (() => Promise<void>)?][] = [
  ["overview", "/"],
  ["console", `/servers/${server}`],
  ["backups", "/backups"],
  ["proxy", "/proxy"],
];
for (const [name, path, prepare] of shots) {
  await page.goto(`${url}${path}`);
  await page.waitForLoadState("networkidle");
  // Let live stats, sparklines and the console fill in.
  await page.waitForTimeout(6000);
  await prepare?.();
  await page.screenshot({ path: `${out}${name}.png` });
  console.log(`${name}.png`);
}
await browser.close();
