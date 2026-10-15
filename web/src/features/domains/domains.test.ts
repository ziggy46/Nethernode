import { describe, expect, it } from "vitest";

import { domainError } from "./queries";

describe("domainError", () => {
  it("accepts domains, with or without a wildcard or trailing dot", () => {
    for (const d of ["example.com", "mc.example.com", "*.example.com", "Example.COM.", "my-server.co.uk"]) {
      expect(domainError(d), d).toBeUndefined();
    }
  });

  it("explains what's wrong", () => {
    expect(domainError("")).toBe("Enter a domain.");
    expect(domainError("10.0.0.5")).toMatch(/IP address/);
    expect(domainError("localhost")).toMatch(/like example.com/);
    expect(domainError("-x.com")).toMatch(/like example.com/);
    expect(domainError("has space.com")).toMatch(/like example.com/);
  });
});
