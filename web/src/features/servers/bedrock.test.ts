import { describe, expect, it } from "vitest";

import { bedrockUdpRange } from "./bedrock";

describe("bedrockUdpRange", () => {
  it("gives each server 32 ports after the last one's", () => {
    expect(bedrockUdpRange(19132)).toEqual([19200, 19231]);
    expect(bedrockUdpRange(19133)).toEqual([19232, 19263]);
    expect(bedrockUdpRange(19132 + 63)).toEqual([19200 + 63 * 32, 19200 + 63 * 32 + 31]);
  });

  it("gives a server from before they had their own ports a range that fits", () => {
    const [from, to] = bedrockUdpRange(30005);
    expect(from).toBe(30105);
    expect(to).toBe(30136);
    expect(bedrockUdpRange(65535)[1]).toBeLessThanOrEqual(65535);
  });
});
