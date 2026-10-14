import { describe, expect, it } from "vitest";

import { describeAgent } from "./agent";

describe("describeAgent", () => {
  it("names common browsers and systems", () => {
    expect(
      describeAgent(
        "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.0 Safari/605.1.15",
      ),
    ).toBe("Safari on macOS");
    expect(
      describeAgent(
        "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0 Safari/537.36 Edg/120.0",
      ),
    ).toBe("Edge on Windows");
    expect(describeAgent("Mozilla/5.0 (X11; Linux x86_64; rv:121.0) Gecko/20100101 Firefox/121.0")).toBe(
      "Firefox on Linux",
    );
    expect(
      describeAgent(
        "Mozilla/5.0 (iPhone; CPU iPhone OS 17_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.0 Mobile/15E148 Safari/604.1",
      ),
    ).toBe("Safari on iOS");
    expect(describeAgent("")).toBe("Unknown device");
  });
});
