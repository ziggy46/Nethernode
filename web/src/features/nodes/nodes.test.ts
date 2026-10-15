import { create } from "@bufbuild/protobuf";
import { describe, expect, it } from "vitest";
import { JoinCodeSchema } from "@/gen/nethernode/v1/nodes_pb";

import { joinCommand } from "./queries";

describe("joinCommand", () => {
  it("runs the panel's image as an agent that joins this panel", () => {
    const join = create(JoinCodeSchema, {
      token: "nnj1.abc",
      image: "ghcr.io/ziggy46/nethernode:0.1",
      expiresAtMs: 0n,
    });
    const command = joinCommand(join, "https://panel.example.com");
    expect(command).toContain("--network host");
    expect(command).toContain("-v /var/run/docker.sock:/var/run/docker.sock");
    expect(command).toContain(
      "ghcr.io/ziggy46/nethernode:0.1 agent --join https://panel.example.com --code nnj1.abc",
    );
  });
});
