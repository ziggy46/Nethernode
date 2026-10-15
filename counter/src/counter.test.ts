import { describe, expect, it } from "vitest";

import { badge, dayNumber, isVersion, parsePing } from "./counter";

const ID = "3f1c2b9a-7d4e-4a6b-9c2d-1e0f5a7b8c9d";

describe("parsePing", () => {
  it("accepts exactly an install ID and a version", () => {
    expect(parsePing(JSON.stringify({ id: ID, version: "0.1.0" }))).toEqual({ id: ID, version: "0.1.0" });
    expect(parsePing(JSON.stringify({ id: ID, version: "1.2.0-rc.1" }))?.version).toBe("1.2.0-rc.1");
  });

  it("refuses anything else", () => {
    for (const body of [
      "not json",
      "[]",
      JSON.stringify({ id: ID }),
      JSON.stringify({ id: "not-a-uuid", version: "0.1.0" }),
      JSON.stringify({ id: ID.toUpperCase(), version: "0.1.0" }),
      JSON.stringify({ id: "3f1c2b9a-7d4e-1a6b-9c2d-1e0f5a7b8c9d", version: "0.1.0" }), // not v4
      JSON.stringify({ id: ID, version: "0.1.0", hostname: "smp.example.com" }),
      JSON.stringify({ id: ID, version: "<script>" }),
      "x".repeat(500),
    ]) {
      expect(parsePing(body), body).toBeNull();
    }
    expect(isVersion("1.2.3.4")).toBe(false);
  });
});

describe("counting", () => {
  it("stores days, not times", () => {
    expect(dayNumber(Date.UTC(2026, 9, 14, 23, 59))).toBe(dayNumber(Date.UTC(2026, 9, 14, 0, 0)));
    expect(dayNumber(Date.UTC(2026, 9, 15))).toBe(dayNumber(Date.UTC(2026, 9, 14)) + 1);
  });

  it("makes a shields.io badge", () => {
    expect(badge(42)).toEqual({ schemaVersion: 1, label: "active installs", message: "42", color: "f2701c" });
  });
});
