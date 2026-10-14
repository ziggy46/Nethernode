import { describe, expect, it } from "vitest";

import { fromCron, parseTime, toCron, type When } from "./when";

describe("when", () => {
  it("round-trips the common choices", () => {
    const cases: When[] = [
      { kind: "daily", hour: 4, minute: 0 },
      { kind: "weekly", day: 0, hour: 23, minute: 30 },
      { kind: "hourly", every: 1, minute: 15 },
      { kind: "hourly", every: 6, minute: 0 },
    ];
    for (const when of cases) expect(fromCron(toCron(when))).toEqual(when);
  });

  it("writes plain cron", () => {
    expect(toCron({ kind: "daily", hour: 4, minute: 0 })).toBe("0 4 * * *");
    expect(toCron({ kind: "weekly", day: 1, hour: 6, minute: 5 })).toBe("5 6 * * 1");
    expect(toCron({ kind: "hourly", every: 6, minute: 0 })).toBe("0 */6 * * *");
  });

  it("leaves anything else as custom", () => {
    for (const cron of ["0 4 1 * *", "*/5 * * * *", "0 4 * * 1-5", "0 */5 * * *", "0 4 * *", "x"]) {
      expect(fromCron(cron)).toEqual({ kind: "custom", cron });
    }
  });

  it("parses time inputs", () => {
    expect(parseTime("04:30")).toEqual({ hour: 4, minute: 30 });
    expect(parseTime("24:00")).toBeNull();
    expect(parseTime("")).toBeNull();
  });
});
