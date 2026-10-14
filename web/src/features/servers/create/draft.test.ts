import { describe, expect, it } from "vitest";

import { curseForgeSlug, slugPreview } from "./draft";

describe("slugPreview", () => {
  it("matches the server's slugs", () => {
    expect(slugPreview("Nether SMP")).toBe("nether-smp");
    expect(slugPreview("  Better MC!! (Forge) ")).toBe("better-mc-forge");
    expect(slugPreview("Ünïcødé")).toBe("n-c-d");
    expect(slugPreview("!!!")).toBe("server");
    expect(slugPreview("a".repeat(50))).toHaveLength(32);
  });
});

describe("curseForgeSlug", () => {
  it("takes slugs or page URLs", () => {
    expect(curseForgeSlug("all-the-mods-9")).toBe("all-the-mods-9");
    expect(curseForgeSlug("https://www.curseforge.com/minecraft/modpacks/All-The-Mods-9/files")).toBe(
      "all-the-mods-9",
    );
  });
});
