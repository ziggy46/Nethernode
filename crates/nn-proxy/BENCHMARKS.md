# Smart Proxy benchmarks

Targets from `08-performance-budget.md`: added p99 latency < 1 ms on
localhost, and 5,000 idle connections < 100 MB RSS (< 20 KB each).

## Results (2026-10-13)

Loopback, release build, proxy in its own process (`examples/standalone`)
routing to an echo backend (`examples/loadgen echo`). "Idle" connections
have completed the handshake and Login Start and are spliced to the
backend. Latency is a 64-byte round trip, so it includes two proxy hops.

| Metric | macOS (Apple Silicon) | Linux (OrbStack VM, 12 cores, kernel 7.2) | Target |
|---|---|---|---|
| Added p99 round-trip latency | 24–41 µs | 50 µs | < 1 ms |
| Proxy RSS, 5,000 idle connections | 26 MB | 24 MB | < 100 MB |
| Per idle connection | ~4.6 KB | ~4.3 KB | < 20 KB |
| Echo throughput vs. direct | 45% (674 MiB/s) | 89% (3,630 MiB/s) | — |

Per-connection CPU before splicing (criterion, `benches/routing.rs`):
parse handshake + normalize hostname + resolve in a 1,000-route table
≈ 100 ns; slug-wildcard resolve ≈ 27 ns.

### Copy strategy

The first version used tokio's `copy_bidirectional_with_sizes`, which keeps
two 64 KiB buffers per connection for its whole life. On Apple Silicon each
touched buffer pins a 16 KiB page, so 5,000 idle players cost **108 MB**,
over budget. The current copy waits for readability before borrowing a
buffer from a shared pool, so idle connections hold none (**26 MB**).

A Linux `splice(2)` zero-copy path (via `tokio-splice2`) was measured
against it on the same machine:

| Linux | Pooled copy | splice(2) |
|---|---|---|
| Added p99 latency | 50.2 µs | 51.5 µs |
| Throughput vs. direct | 89% | 86% |
| RSS, 5,000 idle | 24 MB | 32 MB |

The pooled copy won on throughput and memory with equal latency, so the
splice path was removed rather than kept as an option.

## Reproducing

```sh
cargo build --release -p nn-proxy --examples
T=target/release/examples
$T/loadgen echo 127.0.0.1:25601 &
$T/standalone 127.0.0.1:25600 127.0.0.1:25601 &      # prints its pid
$T/loadgen latency 127.0.0.1:25600 127.0.0.1:25601
$T/loadgen throughput 127.0.0.1:25600 127.0.0.1:25601 2048
$T/loadgen idle 127.0.0.1:25600 5000 30 &           # then: ps -o rss= -p <pid>
cargo bench -p nn-proxy --bench routing
```

On Linux, `scripts/linux-bench.sh` runs the whole set in a container.
