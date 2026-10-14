// Runs the built NetherNode binary for the browser tests: a fresh data
// folder, loopback-only ports, and no Docker (the socket path doesn't exist),
// so everything that doesn't start a Minecraft server can be exercised.
//
//   NN_BIN=../target/release/nethernode node e2e/serve.mjs 18300
import { spawn } from "node:child_process";
import { existsSync, mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const port = process.argv[2] ?? "18300";
const candidates = [process.env.NN_BIN, "../target/release/nethernode", "../target/debug/nethernode"];
const bin = candidates.find((c) => c && existsSync(c));
if (!bin) {
  console.error("Build NetherNode first (cargo build --release -p nethernode) or set NN_BIN.");
  process.exit(1);
}
const data = mkdtempSync(join(tmpdir(), "nn-e2e-"));
const child = spawn(bin, ["serve"], {
  stdio: "inherit",
  env: {
    ...process.env,
    NETHERNODE_DATA__DIR: data,
    NETHERNODE_HTTP__BIND: `127.0.0.1:${port}`,
    NETHERNODE_PROXY__DEFAULT_LISTENER: `127.0.0.1:${Number(port) + 7000}`,
    NETHERNODE_DOCKER__SOCKET: `unix://${join(data, "no-docker.sock")}`,
    RUST_LOG: "warn",
  },
});
const stop = () => {
  child.kill("SIGTERM");
};
process.on("SIGINT", stop);
process.on("SIGTERM", stop);
child.on("exit", (code) => {
  rmSync(data, { recursive: true, force: true });
  process.exit(code ?? 0);
});
