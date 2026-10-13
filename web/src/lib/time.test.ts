import { describe, expect, it } from "vitest";

import { duration, until } from "./time";

describe("duration", () => {
  it("picks sensible units", () => {
    expect(duration(30_000)).toBe("under a minute");
    expect(duration(5 * 60_000)).toBe("5 min");
    expect(duration(3 * 3_600_000 + 20 * 60_000)).toBe("3 h 20 min");
    expect(duration(2 * 86_400_000)).toBe("2 d");
  });
});

describe("until", () => {
  it("counts down to a future time", () => {
    expect(until(61 * 60_000, 0)).toBe("in 1 h 1 min");
    expect(until(10_000, 0)).toBe("in under a minute");
  });
});
