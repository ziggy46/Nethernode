import { describe, expect, it } from "vitest";

import { passwordError, usernameError } from "./validation";

describe("setup validation mirrors the server rules", () => {
  it("accepts valid usernames", () => {
    for (const name of ["abc", "admin", "steve_01", "a.b-c", "x".repeat(32), "  padded  "]) {
      expect(usernameError(name), name).toBeUndefined();
    }
  });

  it("rejects bad usernames", () => {
    for (const name of ["ab", "x".repeat(33), "has space", "emoji🙂", "semi;colon"]) {
      expect(usernameError(name), name).toBeDefined();
    }
  });

  it("enforces password length", () => {
    expect(passwordError("short")).toBeDefined();
    expect(passwordError("long enough")).toBeUndefined();
    expect(passwordError("x".repeat(1025))).toBeDefined();
  });
});
