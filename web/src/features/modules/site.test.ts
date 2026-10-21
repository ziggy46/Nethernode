import { describe, expect, it } from "vitest";

import { beyondCertificate, serverAddresses, siteAddresses, siteError } from "./site";

describe("module sites", () => {
  const routes = [
    { hostname: "*.xtra-pickles.com" },
    { hostname: "play.example.org", serverId: "s1" },
    { hostname: "other.example.org", serverId: "s2" },
    { hostname: "*", serverId: "s1" },
  ];

  it("puts the prefix in front of each of the server's addresses", () => {
    const addresses = serverAddresses({ id: "s1", slug: "mc1" }, routes);
    expect(addresses).toEqual(["play.example.org", "mc1.xtra-pickles.com"]);
    expect(siteAddresses("Map", addresses)).toEqual(["map.play.example.org", "map.mc1.xtra-pickles.com"]);
    expect(siteAddresses("map.example.net", addresses)).toEqual(["map.example.net"]);
    expect(siteAddresses("map", [])).toEqual([]);
  });

  it("checks what's typed", () => {
    expect(siteError("map")).toBeUndefined();
    expect(siteError("map.mc1.example.com")).toBeUndefined();
    for (const bad of ["", "-map", "ma p", "map..x.com", "map_1"]) expect(siteError(bad)).toBeDefined();
  });

  it("stays one level below a domain Cloudflare manages", () => {
    const zones = ["xtra-pickles.com"];
    expect(
      siteAddresses("map", ["mc1.xtra-pickles.com", "a.b.xtra-pickles.com", "xtra-pickles.com"], zones),
    ).toEqual(["map-mc1.xtra-pickles.com", "map-a-b.xtra-pickles.com", "map.xtra-pickles.com"]);
    expect(beyondCertificate("map.mc1.xtra-pickles.com", zones)).toBe(true);
    expect(beyondCertificate("map-mc1.xtra-pickles.com", zones)).toBe(false);
    expect(beyondCertificate("map.example.org", zones)).toBe(false);
  });
});
