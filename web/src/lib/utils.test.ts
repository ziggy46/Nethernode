import { describe, expect, it } from "vitest";

import { cn } from "./utils";

describe("cn", () => {
  it("keeps a named text size alongside a text color", () => {
    expect(cn("truncate text-meta", "text-text-muted")).toBe("truncate text-meta text-text-muted");
    expect(cn("text-meta text-danger", "text-label")).toBe("text-danger text-label");
    expect(cn("text-text-muted", "text-danger")).toBe("text-danger");
  });
});
