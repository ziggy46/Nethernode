import { describe, expect, it } from "vitest";

import { Role } from "@/gen/nethernode/v1/auth_pb";

import { ALL_NAV, canSee, navItemFor } from "./nav";

describe("navItemFor", () => {
  it("maps paths to their section", () => {
    expect(navItemFor("/")?.label).toBe("Overview");
    expect(navItemFor("/servers")?.label).toBe("Servers");
    expect(navItemFor("/servers/abc/console")?.label).toBe("Servers");
    expect(navItemFor("/settings")?.label).toBe("Settings");
    expect(navItemFor("/nope")).toBeUndefined();
  });
});

describe("canSee", () => {
  const visible = (role: Role) => ALL_NAV.filter((item) => canSee(item, role)).map((item) => item.label);
  it("hides the audit log and users from roles below them", () => {
    expect(visible(Role.VIEWER)).not.toContain("Audit log");
    expect(visible(Role.ADMIN)).toContain("Audit log");
    expect(visible(Role.ADMIN)).not.toContain("Users");
    expect(visible(Role.OWNER)).toContain("Users");
  });
});
