import { create } from "@bufbuild/protobuf";
import { describe, expect, it } from "vitest";

import { ListenerSchema, RouteKind, RouteSchema } from "@/gen/nethernode/v1/proxy_pb";

import { dnsRecords, wildcardNeedsPort } from "./dns";

const route = (hostname: string, kind: RouteKind) => create(RouteSchema, { hostname, kind });
const listener = (port: number, enabled = true) => create(ListenerSchema, { port, enabled });

describe("dnsRecords", () => {
  it("asks for an A record per hostname, and SRV records off the default port", () => {
    const routes = [
      route("play.example.com", RouteKind.EXACT),
      route("*.mc.example.com", RouteKind.WILDCARD),
      route("*", RouteKind.DEFAULT),
    ];
    expect(dnsRecords(routes, [listener(25565)]).map((r) => `${r.type} ${r.name}`)).toEqual([
      "A play.example.com",
      "A *.mc.example.com",
    ]);
    const offPort = dnsRecords(routes, [listener(25570), listener(25565, false)]);
    expect(offPort.at(-1)).toEqual({
      type: "SRV",
      name: "_minecraft._tcp.play.example.com",
      value: "0 5 25570 play.example.com",
    });
    expect(wildcardNeedsPort(routes, [listener(25570)])).toBe(true);
    expect(wildcardNeedsPort(routes, [listener(25565)])).toBe(false);
  });

  it("covers names under a domain with one wildcard record", () => {
    const routes = [
      route("smp.example.com", RouteKind.EXACT),
      route("example.com", RouteKind.EXACT),
      route("play.other.net", RouteKind.EXACT),
    ];
    expect(dnsRecords(routes, [listener(25565)], ["example.com"]).map((r) => `${r.type} ${r.name}`)).toEqual([
      "A *.example.com",
      "A example.com",
      "A play.other.net",
    ]);
  });
});
