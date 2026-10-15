/**
 * The color theme. The choice (`system`, `dark` or `light`) is kept in this
 * browser and in the account (Settings › General preferences); `system`
 * follows the device. public/theme.js applies it before the page paints, and
 * this module keeps it up to date afterwards.
 */
import { useSyncExternalStore } from "react";

export type ThemeChoice = "system" | "dark" | "light";
export type Theme = "dark" | "light";

const KEY = "nn.theme";
const listeners = new Set<() => void>();
const lightQuery =
  typeof window !== "undefined" ? window.matchMedia?.("(prefers-color-scheme: light)") : undefined;

/** The choice saved in this browser. */
export function themeChoice(): ThemeChoice {
  try {
    const saved = localStorage.getItem(KEY);
    if (saved === "dark" || saved === "light") return saved;
  } catch {
    // Storage blocked: follow the device.
  }
  return "system";
}

/** The theme a choice comes to on this device right now. */
export function resolveTheme(choice: ThemeChoice, deviceLight = lightQuery?.matches ?? false): Theme {
  if (choice === "system") return deviceLight ? "light" : "dark";
  return choice;
}

function apply() {
  const root = document.documentElement;
  const theme = resolveTheme(themeChoice());
  if (root.dataset.theme !== theme) root.dataset.theme = theme;
  const background = getComputedStyle(root).getPropertyValue("--color-background").trim();
  if (background) document.querySelector('meta[name="theme-color"]')?.setAttribute("content", background);
  for (const listener of listeners) listener();
}

/** Saves a choice in this browser and applies it. */
export function setThemeChoice(choice: ThemeChoice) {
  try {
    if (choice === "system") localStorage.removeItem(KEY);
    else localStorage.setItem(KEY, choice);
  } catch {
    // Storage blocked: it still applies until the page reloads.
  }
  apply();
}

if (typeof window !== "undefined") {
  lightQuery?.addEventListener("change", apply);
  // Another tab changed it.
  window.addEventListener("storage", (event) => {
    if (event.key === KEY || event.key === null) apply();
  });
}

function subscribe(listener: () => void) {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

/** The saved choice, updating when it changes. */
export function useThemeChoice(): ThemeChoice {
  return useSyncExternalStore(subscribe, themeChoice, () => "system");
}

/** The theme on screen, updating when it changes. */
export function useTheme(): Theme {
  return useSyncExternalStore(
    subscribe,
    () => (document.documentElement.dataset.theme === "light" ? "light" : "dark"),
    () => "dark",
  );
}
