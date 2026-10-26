import { describe, expect, it } from "vitest";

import { isReleaseVersion, newerRelease } from "./versions";

const list = [
  { id: "1.21.5", snapshot: false },
  { id: "1.21.4", snapshot: false },
  { id: "1.21.3", snapshot: false },
];

describe("version hints", () => {
  it("offers the newest release to a server that's behind", () => {
    expect(newerRelease(list, "1.21.3")).toBe("1.21.5");
    expect(newerRelease(list, "1.21.4")).toBe("1.21.5");
  });

  it("offers nothing to a server that's current, or on a version it doesn't know", () => {
    expect(newerRelease(list, "1.21.5")).toBeUndefined();
    expect(newerRelease(list, "1.7.10")).toBeUndefined();
    expect(newerRelease([], "1.21.5")).toBeUndefined();
  });

  it("skips snapshots", () => {
    const withSnapshot = [{ id: "26.4-snapshot-1", snapshot: true }, ...list];
    expect(newerRelease(withSnapshot, "1.21.4")).toBe("1.21.5");
  });

  it("only checks mods for what looks like a release", () => {
    expect(isReleaseVersion("1.21.4")).toBe(true);
    expect(isReleaseVersion("26.3")).toBe(true);
    expect(isReleaseVersion("1.21")).toBe(true);
    expect(isReleaseVersion("1.2")).toBe(true);
    expect(isReleaseVersion("1.")).toBe(false);
    expect(isReleaseVersion("LATEST")).toBe(false);
    expect(isReleaseVersion("26.4-snapshot-4")).toBe(false);
  });
});
