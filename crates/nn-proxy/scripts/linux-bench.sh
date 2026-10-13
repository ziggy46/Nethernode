#!/usr/bin/env bash
# Proxy benchmarks on Linux, run inside a container from the repo root:
#
#   docker run --rm --ulimit nofile=65536:65536 \
#     -v "$PWD":/src:ro -v nn-bench-target:/target \
#     rust:1.96-bookworm bash /src/crates/nn-proxy/scripts/linux-bench.sh
#
# Prints added latency, echo throughput, and proxy RSS with 5,000 idle
# logged-in connections (see crates/nn-proxy/BENCHMARKS.md).
set -euo pipefail
cd /src
export CARGO_TARGET_DIR=/target/bench
cargo build --release --locked -q -p nn-proxy --examples
T=$CARGO_TARGET_DIR/release/examples

$T/loadgen echo 127.0.0.1:25601 >/dev/null 2>&1 &
echo_pid=$!
$T/standalone 127.0.0.1:25600 127.0.0.1:25601 >/dev/null 2>&1 &
proxy_pid=$!
trap 'kill $proxy_pid $echo_pid 2>/dev/null' EXIT
sleep 1

echo "cores: $(nproc), kernel: $(uname -r)"
$T/loadgen latency 127.0.0.1:25600 127.0.0.1:25601 20000
$T/loadgen throughput 127.0.0.1:25600 127.0.0.1:25601 2048
echo "proxy RSS, idle: $(ps -o rss= -p $proxy_pid) KB"
$T/loadgen idle 127.0.0.1:25600 5000 8 &
idle_pid=$!
sleep 6
echo "proxy RSS, 5000 idle connections: $(ps -o rss= -p $proxy_pid) KB"
wait $idle_pid
