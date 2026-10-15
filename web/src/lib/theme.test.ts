import { afterEach, describe, expect, it } from "vitest";

import { resolveTheme, setThemeChoice, themeChoice } from "./theme";

afterEach(() => {
  localStorage.clear();
  delete document.documentElement.dataset.theme;
});

describe("theme", () => {
  it("follows the device only for the system choice", () => {
    expect(resolveTheme("system", true)).toBe("light");
    expect(resolveTheme("system", false)).toBe("dark");
    expect(resolveTheme("dark", true)).toBe("dark");
    expect(resolveTheme("light", false)).toBe("light");
  });

  it("saves the choice and applies it to the page", () => {
    expect(themeChoice()).toBe("system");
    setThemeChoice("light");
    expect(themeChoice()).toBe("light");
    expect(document.documentElement.dataset.theme).toBe("light");
    setThemeChoice("dark");
    expect(document.documentElement.dataset.theme).toBe("dark");
    setThemeChoice("system");
    expect(localStorage.getItem("nn.theme")).toBeNull();
  });

  it("ignores a value it doesn't know", () => {
    localStorage.setItem("nn.theme", "sepia");
    expect(themeChoice()).toBe("system");
  });
});
