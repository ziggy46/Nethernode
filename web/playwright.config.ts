import { defineConfig, devices } from "@playwright/test";

/** The built NetherNode binary serving its embedded UI; see e2e/serve.mjs. */
const PORT = Number(process.env.NN_E2E_PORT ?? 18300);

export default defineConfig({
  testDir: "./e2e",
  // One panel, one owner: the specs build on each other in order.
  fullyParallel: false,
  workers: 1,
  forbidOnly: Boolean(process.env.CI),
  reporter: process.env.CI ? [["github"], ["html", { open: "never" }]] : "list",
  use: {
    baseURL: `http://127.0.0.1:${PORT}`,
    trace: "retain-on-failure",
  },
  projects: [{ name: "chromium", use: { ...devices["Desktop Chrome"] } }],
  webServer: {
    command: `node e2e/serve.mjs ${PORT}`,
    url: `http://127.0.0.1:${PORT}/healthz`,
    reuseExistingServer: false,
    timeout: 60_000,
  },
});
