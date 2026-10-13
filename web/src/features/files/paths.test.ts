import { describe, expect, it } from "vitest";

import {
  ago,
  baseName,
  crumbs,
  formatSize,
  joinPath,
  languageFor,
  parentPath,
  runningEditWarning,
  validName,
} from "./paths";

describe("paths", () => {
  it("joins and splits", () => {
    expect(joinPath("", "a")).toBe("a");
    expect(joinPath("a/b", "c")).toBe("a/b/c");
    expect(parentPath("a/b/c")).toBe("a/b");
    expect(parentPath("a")).toBe("");
    expect(baseName("a/b/c.yml")).toBe("c.yml");
    expect(crumbs("world/region")).toEqual([
      { name: "world", path: "world" },
      { name: "region", path: "world/region" },
    ]);
    expect(crumbs("")).toEqual([]);
  });

  it("validates names", () => {
    expect(validName("plugins")).toBeNull();
    expect(validName(" ")).not.toBeNull();
    expect(validName("..")).not.toBeNull();
    expect(validName("a/b")).not.toBeNull();
  });

  it("picks editor languages", () => {
    expect(languageFor("config/paper-global.yml")).toBe("yaml");
    expect(languageFor("ops.json")).toBe("json");
    expect(languageFor("server.properties")).toBe("properties");
    expect(languageFor("config/fabric.toml")).toBe("toml");
    expect(languageFor("eula.txt")).toBe("plain");
    expect(languageFor("Makefile")).toBe("plain");
  });

  it("warns about files the server rewrites", () => {
    expect(runningEditWarning("ops.json")).toContain("rewrites");
    expect(runningEditWarning("server.properties")).toContain("restart");
    expect(runningEditWarning("plugins/x/config.yml")).toBeNull();
  });

  it("formats sizes", () => {
    expect(formatSize(512)).toBe("512 B");
    expect(formatSize(1536)).toBe("1.5 KB");
    expect(formatSize(25 * 1024 * 1024)).toBe("25 MB");
  });
});

describe("ago", () => {
  it("rounds to friendly units", () => {
    const now = 10_000_000_000;
    expect(ago(now - 10_000, now)).toBe("just now");
    expect(ago(now - 5 * 60_000, now)).toBe("5 min ago");
    expect(ago(now - 3 * 3_600_000, now)).toBe("3 h ago");
    expect(ago(now - 86_400_000, now)).toBe("1 day ago");
    expect(ago(0, now)).toBe("");
  });
});
