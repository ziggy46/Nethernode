import { describe, expect, it } from "vitest";

import { looksLikeIpAddress } from "./route-dialog";

describe("looksLikeIpAddress", () => {
  it("detects IP literals", () => {
    for (const ip of ["127.0.0.1", " 203.0.113.5 ", "::1", "[2001:db8::1]"])
      expect(looksLikeIpAddress(ip), ip).toBe(true);
    for (const host of ["play.example.com", "*", "*.example.com", "forge.127.0.0.1.nip.io", ""]) {
      expect(looksLikeIpAddress(host), host).toBe(false);
    }
  });
});
