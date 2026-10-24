import { describe, expect, it } from "vitest";

import { curseForgeSlug, domainAddress, newDraft, slugPreview, softwareInfo, subdomainError } from "./draft";

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

describe("domainAddress", () => {
  const base = newDraft();
  it("uses the server's name unless a subdomain is given", () => {
    expect(domainAddress({ ...base, name: "Nether SMP" }, ["example.com"])).toBe("nether-smp.example.com");
    expect(domainAddress({ ...base, name: "Nether SMP", subdomain: "SMP" }, ["example.com"])).toBe(
      "smp.example.com",
    );
    expect(domainAddress({ ...base, subdomain: "smp", domain: "mc.example.com" }, ["example.com"])).toBe(
      "smp.mc.example.com",
    );
    expect(domainAddress({ ...base, apex: true }, ["example.com"])).toBe("example.com");
    expect(domainAddress(base, [])).toBeNull();
  });

  it("checks subdomains", () => {
    expect(subdomainError("")).toBeUndefined();
    expect(subdomainError("smp")).toBeUndefined();
    expect(subdomainError("eu.smp")).toBeUndefined();
    expect(subdomainError("-smp")).toBeDefined();
    expect(subdomainError("my server")).toBeDefined();
  });
});

describe("softwareInfo", () => {
  it("knows the software under More too", () => {
    expect(softwareInfo("purpur")?.memoryMb).toBe(4096);
    expect(softwareInfo("quilt")?.memoryMb).toBe(4096);
    expect(softwareInfo("paper")?.name).toBe("Paper");
  });

  it("offers Bedrock with a small memory default", () => {
    expect(softwareInfo("bedrock")?.name).toBe("Bedrock");
    expect(softwareInfo("bedrock")?.memoryMb).toBe(2048);
  });
});
