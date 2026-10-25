import { describe, expect, it } from "vitest";

import { formatBytes, formatUptime, gb, loaderName, memoryLimitBytes, versionLabel } from "./format";

describe("formatting", () => {
  it("formats bytes", () => {
    expect(formatBytes(512 * 1024 ** 2)).toBe("512 MB");
    expect(formatBytes(3.24 * 1024 ** 3)).toBe("3.2 GB");
    expect(formatBytes(32 * 1024 ** 3)).toBe("32 GB");
    expect(gb(6.4 * 1024 ** 3)).toBe("6.4");
  });

  it("formats uptime", () => {
    expect(formatUptime(30_000)).toBe("under a minute");
    expect(formatUptime(5 * 60_000)).toBe("5m");
    expect(formatUptime((2 * 60 + 34) * 60_000)).toBe("2h 34m");
    expect(formatUptime((3 * 24 + 4) * 3_600_000)).toBe("3d 4h");
  });

  it("names loaders and versions", () => {
    expect(loaderName("neoforge")).toBe("NeoForge");
    expect(loaderName("custom")).toBe("custom");
    expect(versionLabel("LATEST")).toBe("Latest release");
    expect(versionLabel("1.21.4")).toBe("1.21.4");
  });
});

describe("memoryLimitBytes", () => {
  it("adds more room for mods", () => {
    const mb = (n: number) => n * 1024 * 1024;
    expect(memoryLimitBytes(4096)).toBe(mb(5120));
    expect(memoryLimitBytes(4096, "paper")).toBe(mb(5120));
    expect(memoryLimitBytes(4096, "fabric")).toBe(mb(6144));
    expect(memoryLimitBytes(6144, "curseforge")).toBe(mb(6144 + 6144 * 0.4));
  });
});
