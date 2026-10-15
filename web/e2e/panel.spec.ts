import { expect, test } from "@playwright/test";

import { freshPage, nextStep, OWNER, signIn, totp, watchCsp } from "./helpers";

// One panel for the whole file, built up step by step.
test.describe.configure({ mode: "serial" });

let builderPassword = "";
let totpSecret = "";
let recoveryCodes: string[] = [];

test("first run creates the owner and explains that Docker is missing", async ({ page }) => {
  const noCsp = watchCsp(page);
  await page.goto("/");
  await expect(page).toHaveURL(/\/setup$/);
  await page.getByLabel("Username").fill(OWNER.username);
  await page.getByLabel("Password", { exact: true }).fill(OWNER.password);
  await page.getByLabel("Confirm password").fill(OWNER.password);
  await page.getByRole("button", { name: /Create/ }).click();
  // Next, the optional domain.
  await expect(page.getByRole("heading", { name: "Do you have a domain?" })).toBeVisible();
  await page.getByLabel("Your domain").fill("Example.COM");
  await expect(page.getByText("smp.example.com")).toBeVisible();
  await page.getByRole("button", { name: "Save and continue" }).click();
  await expect(page.getByRole("heading", { name: "Your worlds, under control." })).toBeVisible();
  await expect(page.getByText("Docker isn't available")).toBeVisible();
  noCsp();
});

test("every page loads without breaking the content security policy", async ({ page }) => {
  const noCsp = watchCsp(page);
  await signIn(page, OWNER.username, OWNER.password);
  await expect(page).toHaveURL(/\/$/);
  const response = await page.goto("/backups");
  expect(response?.headers()["content-security-policy"]).toContain("script-src 'self'");
  for (const path of [
    "/servers",
    "/players",
    "/proxy",
    "/modpacks",
    "/backups",
    "/host",
    "/audit",
    "/users",
    "/settings",
  ]) {
    await page.goto(path);
    await expect(page.locator("main h1").first()).toBeVisible();
  }
  noCsp();
});

test("the domain from setup is in Settings and the DNS helper", async ({ page }) => {
  await signIn(page, OWNER.username, OWNER.password);
  await page.goto("/settings");
  await expect(page.getByRole("listitem").filter({ hasText: "example.com" }).first()).toBeVisible();
  await page.getByLabel("Add a domain").fill("not a domain");
  await page.getByRole("button", { name: "Add domain" }).click();
  await expect(page.getByText("Enter a domain like example.com")).toBeVisible();
  await page.getByLabel("Add a domain").fill("mc.example.net");
  await page.getByRole("button", { name: "Add domain" }).click();
  await expect(page.getByText("mc.example.net added.")).toBeVisible();
});

test("pages without servers say so plainly", async ({ page }) => {
  await signIn(page, OWNER.username, OWNER.password);
  await page.goto("/backups");
  await expect(page.getByText("No backups yet")).toBeVisible();
  await expect(page.getByText("Nothing scheduled")).toBeVisible();
  await page.goto("/proxy");
  await expect(page.getByText("No routes yet")).toBeVisible();
});

test("an owner adds a user who must choose their own password", async ({ page, browser }) => {
  await signIn(page, OWNER.username, OWNER.password);
  await page.goto("/users");
  await page.getByRole("button", { name: "Add user" }).click();
  await page.getByLabel("Username").fill("builder");
  await page.getByLabel("Role").selectOption({ label: "Moderator" });
  await page.getByRole("button", { name: "Add user" }).last().click();
  const reveal = page.getByRole("dialog", { name: /One-time password for builder/ });
  await expect(reveal).toBeVisible();
  builderPassword = (await reveal.locator("p.select-all").textContent())?.trim() ?? "";
  expect(builderPassword.length).toBeGreaterThan(10);
  await reveal.getByRole("button", { name: "Done" }).click();
  await expect(page.getByText("Hasn't chosen a password yet")).toBeVisible();

  const other = await freshPage(browser);
  await signIn(other, "builder", builderPassword);
  await expect(other.getByRole("heading", { name: "Choose your password" })).toBeVisible();
  await other.getByLabel("One-time password").fill(builderPassword);
  await other.getByLabel("New password", { exact: true }).fill("builder password 1");
  await other.getByLabel("New password again").fill("builder password 1");
  await other.getByRole("button", { name: "Change password" }).click();
  await expect(other.getByRole("heading", { name: "Your worlds, under control." })).toBeVisible();
  // Moderators don't see owner pages.
  await expect(other.getByRole("link", { name: "Users" })).toHaveCount(0);
  await expect(other.getByRole("link", { name: "Audit log" })).toHaveCount(0);
});

test("two-factor sign-in with an app code, then a recovery code", async ({ page, browser }) => {
  await signIn(page, OWNER.username, OWNER.password);
  await page.goto("/settings");
  await page.getByRole("button", { name: "Set up…" }).click();
  const setup = page.getByRole("dialog", { name: "Set up two-factor sign-in" });
  await expect(setup.getByRole("img", { name: /QR code/ })).toBeVisible();
  totpSecret = (await setup.getByRole("button", { name: /^[A-Z2-7 ]{30,}$/ }).textContent())?.trim() ?? "";
  await setup.getByLabel("Code from the app").fill(totp(totpSecret));
  await setup.getByRole("button", { name: "Turn on" }).click();
  const saved = page.getByRole("dialog", { name: "Save your recovery codes" });
  await expect(saved).toBeVisible();
  recoveryCodes = (await saved.locator("li").allTextContents()).map((c) => c.trim());
  expect(recoveryCodes).toHaveLength(10);
  await saved.getByRole("button", { name: "I've saved them" }).click();

  // The code used to turn it on can't be used again, so wait for the next one.
  await nextStep();
  const laptop = await freshPage(browser);
  await signIn(laptop, OWNER.username, OWNER.password);
  await expect(laptop.getByText("Two-factor sign-in is on for this account.")).toBeVisible();
  await laptop.getByLabel("Authenticator code").fill(totp(totpSecret));
  await laptop.getByRole("button", { name: "Verify" }).click();
  await expect(laptop.getByRole("heading", { name: "Your worlds, under control." })).toBeVisible();

  const phoneLost = await freshPage(browser);
  await signIn(phoneLost, OWNER.username, OWNER.password);
  await phoneLost.getByRole("button", { name: /Use a recovery code/ }).click();
  await phoneLost.getByLabel("Recovery code").fill(recoveryCodes[0] ?? "");
  await phoneLost.getByRole("button", { name: "Verify" }).click();
  await expect(phoneLost.getByRole("heading", { name: "Your worlds, under control." })).toBeVisible();
});

test("the audit log records what happened and filters it", async ({ browser }) => {
  const page = await freshPage(browser);
  await signIn(page, OWNER.username, OWNER.password);
  await page.getByRole("button", { name: /Use a recovery code/ }).click();
  await page.getByLabel("Recovery code").fill(recoveryCodes[1] ?? "");
  await page.getByRole("button", { name: "Verify" }).click();
  await page.goto("/audit");
  await expect(page.getByRole("cell", { name: /Added a user · builder/ })).toBeVisible();
  await expect(page.getByRole("cell", { name: /Turned on two-factor sign-in/ })).toBeVisible();
  await page.getByLabel("What").selectOption({ label: "Users" });
  await expect(page.getByRole("cell", { name: /Turned on two-factor sign-in/ })).toHaveCount(0);
  await expect(page.getByRole("cell", { name: /Added a user · builder/ })).toBeVisible();
});

test("Discord settings refuse a URL that isn't a Discord webhook", async ({ browser }) => {
  const page = await freshPage(browser);
  await signIn(page, OWNER.username, OWNER.password);
  await page.getByRole("button", { name: /Use a recovery code/ }).click();
  await page.getByLabel("Recovery code").fill(recoveryCodes[2] ?? "");
  await page.getByRole("button", { name: "Verify" }).click();
  await page.goto("/settings");
  await page.getByLabel("Webhook URL").fill("https://example.com/api/webhooks/1/abc");
  await page.getByRole("button", { name: "Save", exact: true }).first().click();
  await expect(page.getByText(/Paste a Discord webhook URL/)).toBeVisible();
});
