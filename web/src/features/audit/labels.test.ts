import { describe, expect, it } from "vitest";

import { actionLabel, activityTitle, detailPairs } from "./labels";

describe("audit labels", () => {
  it("names actions and falls back to the raw name", () => {
    expect(actionLabel("server.start")).toBe("Started a server");
    expect(actionLabel("something.new")).toBe("something.new");
    expect(activityTitle("server.restart", "Nether SMP")).toBe("Nether SMP restarted");
  });

  it("turns details into readable pairs", () => {
    expect(
      detailPairs('{"name":"Steve","reason":"griefing","files":["a.jar","b.jar"],"ip":true,"empty":""}'),
    ).toEqual([
      ["reason", "griefing"],
      ["files", "a.jar, b.jar"],
      ["ip", "true"],
    ]);
    expect(detailPairs("not json")).toEqual([]);
    expect(detailPairs("[1,2]")).toEqual([]);
  });
});
