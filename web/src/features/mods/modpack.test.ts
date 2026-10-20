import { describe, expect, it } from "vitest";

import { Platform } from "@/gen/nethernode/v1/catalog_pb";

import { compareMinecraft, minecraftUpgrade, newestMinecraft, versionPage } from "./modpack";

describe("modpack helpers", () => {
  it("orders Minecraft versions by number", () => {
    expect(compareMinecraft("1.21.10", "1.21.4")).toBe(1);
    expect(compareMinecraft("1.21", "1.21.1")).toBe(-1);
    expect(compareMinecraft("1.20.1", "1.20.1")).toBe(0);
    expect(newestMinecraft(["1.21.1", "1.21.10", "1.21.4"])).toBe("1.21.10");
    expect(newestMinecraft([])).toBeUndefined();
  });

  it("only reports a newer Minecraft version", () => {
    expect(minecraftUpgrade("1.21.1", ["1.21.4"])).toBe("1.21.4");
    expect(minecraftUpgrade("1.21.1", ["1.21.1"])).toBeUndefined();
    expect(minecraftUpgrade("LATEST", ["1.21.4"])).toBeUndefined();
  });

  it("links each platform's version page", () => {
    expect(versionPage(Platform.MODRINTH, "https://modrinth.com/modpack/p", "abc")).toBe(
      "https://modrinth.com/modpack/p/version/abc",
    );
    expect(versionPage(Platform.CURSEFORGE, "https://www.curseforge.com/minecraft/modpacks/p/", "12")).toBe(
      "https://www.curseforge.com/minecraft/modpacks/p/files/12",
    );
    expect(versionPage(Platform.MODRINTH, "", "abc")).toBeUndefined();
  });
});
