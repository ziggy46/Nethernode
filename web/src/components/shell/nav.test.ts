import { describe, expect, it } from "vitest";

import { navItemFor } from "./nav";

describe("navItemFor", () => {
  it("maps paths to their section", () => {
    expect(navItemFor("/")?.label).toBe("Overview");
    expect(navItemFor("/servers")?.label).toBe("Servers");
    expect(navItemFor("/servers/abc/console")?.label).toBe("Servers");
    expect(navItemFor("/settings")?.label).toBe("Settings");
    expect(navItemFor("/nope")).toBeUndefined();
  });
});
