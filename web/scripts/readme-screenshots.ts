/**
 * Captures the README screenshots from a running panel with some servers.
 *
 *   NN_URL=http://127.0.0.1:8080 NN_USER=admin NN_PASSWORD=… NN_SERVER=<server id> \
 *     NN_INVENTORY_SERVER=<server id> NN_PLAYER=<name> node scripts/readme-screenshots.ts
 *
 * Use an account whose Appearance is dark (or follows the device): the
 * panel applies the account's own choice at sign-in. NN_SERVER is a
 * running server with players online (its console and
 * performance); NN_INVENTORY_SERVER and NN_PLAYER (optional) a player
 * online there whose live inventory is shown. Writes PNGs to
 * ../assets/screenshots/.
 */
import { chromium, type Page } from "@playwright/test";

const url = process.env.NN_URL ?? "http://127.0.0.1:8080";
const user = process.env.NN_USER;
const password = process.env.NN_PASSWORD;
const server = process.env.NN_SERVER;
const inventoryServer = process.env.NN_INVENTORY_SERVER;
const player = process.env.NN_PLAYER;
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

/** Opens a server tab by name. */
const tab = (page: Page, name: string) => page.getByRole("tab", { name, exact: true }).click();

type Shot = { name: string; path: string; prepare?: (page: Page) => Promise<void>; element?: string };
const shots: Shot[] = [
  { name: "overview", path: "/" },
  {
    name: "console",
    path: `/servers/${server}`,
    // Show command completion with the players online.
    prepare: async (page) => {
      await page.getByRole("button", { name: "Clear the console view" }).click();
      const input = page.getByRole("combobox", { name: "Console command" });
      await input.fill("list");
      await input.press("Enter");
      await page.waitForTimeout(1500);
      await input.pressSequentially("gamemode creative ");
      await page.waitForTimeout(800);
    },
  },
  {
    name: "performance",
    path: `/servers/${server}`,
    element: "section:has(h2:text-is('Performance'))",
  },
  ...(inventoryServer && player
    ? [
        {
          name: "inventory",
          path: `/servers/${inventoryServer}`,
          prepare: async (page: Page) => {
            await tab(page, "Players");
            await page.getByText(player, { exact: true }).first().click();
            await page.getByRole("tab", { name: "Inventory" }).click();
            await page.waitForTimeout(5000);
          },
        },
      ]
    : []),
  { name: "backups", path: "/backups" },
  { name: "proxy", path: "/proxy" },
  {
    name: "modpacks",
    path: "/modpacks",
    // A steady, recognizable set rather than whatever is trending.
    prepare: async (page) => {
      await page.getByRole("searchbox").or(page.getByPlaceholder("Search modpacks")).first().fill("Create");
      await page.waitForTimeout(3000);
    },
  },
];
for (const { name, path, prepare, element } of shots) {
  await page.goto(`${url}${path}`);
  await page.waitForLoadState("networkidle");
  // Let live stats, sparklines and the console fill in.
  await page.waitForTimeout(6000);
  await prepare?.(page);
  if (element) await page.locator(element).screenshot({ path: `${out}${name}.png` });
  else await page.screenshot({ path: `${out}${name}.png` });
  console.log(`${name}.png`);
}
await browser.close();
