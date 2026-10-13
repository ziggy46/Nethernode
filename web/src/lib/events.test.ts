import { QueryClient } from "@tanstack/react-query";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { createInvalidator } from "./events";

describe("createInvalidator", () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => vi.useRealTimers());

  it("coalesces a burst of events into one refetch per query", () => {
    const client = new QueryClient();
    const spy = vi.spyOn(client, "invalidateQueries");
    const invalidator = createInvalidator(client, 400);
    for (let i = 0; i < 50; i++) invalidator.note("proxy_connection_closed");
    invalidator.note("player_joined");
    invalidator.note("proxy_listener_changed");
    invalidator.note("something_else");
    expect(spy).not.toHaveBeenCalled();
    vi.advanceTimersByTime(400);
    expect(spy).toHaveBeenCalledTimes(3);
    expect(spy).toHaveBeenCalledWith({ queryKey: ["proxy", "connections"] });
    expect(spy).toHaveBeenCalledWith({ queryKey: ["players"] });
    expect(spy).toHaveBeenCalledWith({ queryKey: ["proxy", "listeners"] });
  });

  it("lagged refetches everything once", () => {
    const client = new QueryClient();
    const spy = vi.spyOn(client, "invalidateQueries");
    const invalidator = createInvalidator(client, 400);
    invalidator.note("lagged");
    invalidator.note("player_left");
    vi.advanceTimersByTime(400);
    expect(spy).toHaveBeenCalledTimes(1);
    expect(spy).toHaveBeenCalledWith();
  });
});
