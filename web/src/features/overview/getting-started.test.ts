import { describe, expect, it } from "vitest";

import { ListenerState } from "@/gen/nethernode/v1/proxy_pb";

import { joinAddress, listenerStatus } from "./getting-started";

const listener = (state: ListenerState, port = 25565, error = "") => ({ state, port, error });

describe("getting started", () => {
  it("says whether the proxy is taking players", () => {
    expect(listenerStatus([listener(ListenerState.LISTENING)])).toEqual({ kind: "listening", port: 25565 });
    expect(
      listenerStatus([
        listener(ListenerState.FAILED, 25565, "address in use"),
        listener(ListenerState.LISTENING, 25566),
      ]),
    ).toEqual({ kind: "listening", port: 25566 });
    expect(listenerStatus([listener(ListenerState.FAILED, 25565, "address in use")])).toEqual({
      kind: "failed",
      port: 25565,
      error: "address in use",
    });
    expect(listenerStatus([listener(ListenerState.DISABLED)])).toEqual({ kind: "off" });
    expect(listenerStatus([])).toEqual({ kind: "off" });
  });

  it("suggests an address players could type", () => {
    expect(joinAddress([], [])).toBeUndefined();
    expect(joinAddress([], ["example.com"])).toBe("smp.example.com");
    expect(joinAddress([{ slug: "creative", hostnames: [] }], ["example.com"])).toBe("creative.example.com");
    expect(
      joinAddress(
        [
          { slug: "a", hostnames: [] },
          { slug: "b", hostnames: ["play.example.org"] },
        ],
        ["example.com"],
      ),
    ).toBe("play.example.org");
  });
});
