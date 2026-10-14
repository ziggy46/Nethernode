import { createHmac } from "node:crypto";

import { type Browser, expect, type Page } from "@playwright/test";

export const OWNER = { username: "owner", password: "correct horse battery staple" };

/** The current 6-digit code for a base32 secret (RFC 6238). */
export function totp(secret: string, at = Date.now()): string {
  const alphabet = "ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
  let bits = "";
  for (const c of secret.replace(/\s/g, "").toUpperCase())
    bits += alphabet.indexOf(c).toString(2).padStart(5, "0");
  const key = Buffer.alloc(Math.floor(bits.length / 8));
  for (let i = 0; i < key.length; i++) key[i] = Number.parseInt(bits.slice(i * 8, i * 8 + 8), 2);
  const step = Buffer.alloc(8);
  step.writeBigUInt64BE(BigInt(Math.floor(at / 30_000)));
  const h = createHmac("sha1", key).update(step).digest();
  const o = (h[19] ?? 0) & 15;
  const v = (h.readUInt32BE(o) & 0x7fffffff) % 1_000_000;
  return String(v).padStart(6, "0");
}

/** Waits until the next 30-second step, so a fresh code can be used. */
export async function nextStep(): Promise<void> {
  const wait = 30_000 - (Date.now() % 30_000) + 500;
  await new Promise((r) => setTimeout(r, wait));
}

export async function signIn(page: Page, username: string, password: string) {
  await page.goto("/login");
  await page.getByLabel("Username").fill(username);
  await page.getByLabel("Password", { exact: true }).fill(password);
  const answered = page.waitForResponse((r) => r.url().includes("AuthService/Login"));
  await page.getByRole("button", { name: "Sign in" }).click();
  expect((await answered).ok()).toBe(true);
  // Signed in (left the login page), or asked for a second factor.
  await expect(
    page
      .getByRole("button", { name: "Verify" })
      .or(page.getByRole("heading", { level: 1 }).filter({ hasNotText: "Welcome back" })),
  ).toBeVisible();
}

/** A fresh browser (no cookies), for signing in as someone else. */
export async function freshPage(browser: Browser): Promise<Page> {
  const context = await browser.newContext();
  return context.newPage();
}

/** Fails the test if the page reports a Content Security Policy violation. */
export function watchCsp(page: Page): () => void {
  const violations: string[] = [];
  page.on("console", (msg) => {
    if (msg.text().includes("Content Security Policy")) violations.push(msg.text());
  });
  return () => expect(violations, violations.join("\n")).toEqual([]);
}
