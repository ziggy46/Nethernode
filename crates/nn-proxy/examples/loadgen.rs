//! Load generator for proxy benchmarks.
//!
//! - `echo <listen>`: a TCP echo server to act as the backend.
//! - `latency <proxy> <direct> [round_trips]`: round-trip latency through
//!   the proxy vs. straight to the backend; prints p50/p99/p999 and the
//!   p99 the proxy adds.
//! - `idle <proxy> <connections> <hold_secs>`: opens many logged-in
//!   connections through the proxy and holds them (measure proxy RSS meanwhile).
//! - `throughput <proxy> <direct> [megabytes]`: bulk transfer through an
//!   echo backend, through the proxy vs. direct.

use std::net::SocketAddr;
use std::time::{Duration, Instant};

use nn_mcproto::codec::frame;
use nn_mcproto::{Handshake, LoginStart, NextState};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

const PROTOCOL: i32 = 777;
const MESSAGE: [u8; 64] = [0x42; 64];

fn prelude() -> Vec<u8> {
    let hs = Handshake {
        protocol_version: PROTOCOL,
        server_address: "bench.example.com".into(),
        server_port: 25565,
        next_state: NextState::Login,
    };
    let mut out = frame(&hs.encode());
    out.extend(frame(
        &LoginStart {
            username: "Bench".into(),
            uuid: Some(Default::default()),
        }
        .encode(PROTOCOL),
    ));
    out
}

/// Connects and logs in; the echo backend returns the prelude, which is consumed.
async fn open(addr: SocketAddr) -> std::io::Result<TcpStream> {
    let mut stream = TcpStream::connect(addr).await?;
    stream.set_nodelay(true)?;
    let prelude = prelude();
    stream.write_all(&prelude).await?;
    let mut echoed = vec![0; prelude.len()];
    stream.read_exact(&mut echoed).await?;
    Ok(stream)
}

async fn round_trips(addr: SocketAddr, n: usize) -> Vec<Duration> {
    let mut stream = open(addr).await.expect("connect");
    let mut buf = [0u8; MESSAGE.len()];
    // Warm up.
    for _ in 0..200 {
        stream.write_all(&MESSAGE).await.unwrap();
        stream.read_exact(&mut buf).await.unwrap();
    }
    let mut samples = Vec::with_capacity(n);
    for _ in 0..n {
        let t = Instant::now();
        stream.write_all(&MESSAGE).await.unwrap();
        stream.read_exact(&mut buf).await.unwrap();
        samples.push(t.elapsed());
    }
    samples.sort();
    samples
}

fn pct(samples: &[Duration], p: f64) -> Duration {
    let idx = ((samples.len() as f64 * p).ceil() as usize).clamp(1, samples.len()) - 1;
    samples[idx]
}

/// Sends `mb` MiB and reads it back concurrently; returns MiB/s.
async fn throughput(addr: SocketAddr, mb: usize) -> f64 {
    let stream = open(addr).await.expect("connect");
    let (mut r, mut w) = stream.into_split();
    let total = mb << 20;
    let started = Instant::now();
    let writer = tokio::spawn(async move {
        let chunk = vec![0x5a; 64 * 1024];
        let mut sent = 0;
        while sent < total {
            let n = chunk.len().min(total - sent);
            w.write_all(&chunk[..n]).await.unwrap();
            sent += n;
        }
        w
    });
    let mut buf = vec![0; 64 * 1024];
    let mut received = 0;
    while received < total {
        let n = r.read(&mut buf).await.unwrap();
        assert!(n > 0, "closed early");
        received += n;
    }
    drop(writer.await);
    mb as f64 / started.elapsed().as_secs_f64()
}

async fn echo(listen: SocketAddr) {
    let listener = TcpListener::bind(listen).await.expect("bind");
    println!("echo listening on {listen}");
    loop {
        let (mut stream, _) = listener.accept().await.expect("accept");
        tokio::spawn(async move {
            let _ = stream.set_nodelay(true);
            let (mut r, mut w) = stream.split();
            let _ = tokio::io::copy(&mut r, &mut w).await;
        });
    }
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let addr = |i: usize| args[i].parse::<SocketAddr>().expect("address");
    match args.first().map(String::as_str) {
        Some("echo") => echo(addr(1)).await,
        Some("latency") => {
            let n = args.get(3).and_then(|v| v.parse().ok()).unwrap_or(20_000);
            let direct = round_trips(addr(2), n).await;
            let proxied = round_trips(addr(1), n).await;
            for (name, s) in [("direct", &direct), ("proxied", &proxied)] {
                println!(
                    "{name:>8}: p50 {:>7.1?}  p99 {:>7.1?}  p999 {:>7.1?}  ({n} round trips, 64 B)",
                    pct(s, 0.50),
                    pct(s, 0.99),
                    pct(s, 0.999)
                );
            }
            let added = pct(&proxied, 0.99).saturating_sub(pct(&direct, 0.99));
            println!("added p99 latency: {added:.1?} (round trip, i.e. two proxy hops)");
        }
        Some("throughput") => {
            let mb = args.get(3).and_then(|v| v.parse().ok()).unwrap_or(1024);
            let direct = throughput(addr(2), mb).await;
            let proxied = throughput(addr(1), mb).await;
            println!("  direct: {direct:>8.0} MiB/s echoed ({mb} MiB each way)");
            println!(
                " proxied: {proxied:>8.0} MiB/s echoed ({:.0}% of direct)",
                proxied / direct * 100.0
            );
        }
        Some("idle") => {
            let count: usize = args[2].parse().expect("connections");
            let hold: u64 = args[3].parse().expect("hold seconds");
            let mut conns = Vec::with_capacity(count);
            let started = Instant::now();
            for i in 0..count {
                match open(addr(1)).await {
                    Ok(stream) => conns.push(stream),
                    Err(e) => {
                        eprintln!("connection {i} failed: {e}");
                        break;
                    }
                }
            }
            println!(
                "{} connections open in {:.1?}; holding {hold}s",
                conns.len(),
                started.elapsed()
            );
            tokio::time::sleep(Duration::from_secs(hold)).await;
        }
        _ => eprintln!(
            "usage: loadgen echo <listen> | latency <proxy> <direct> [n] | idle <proxy> <conns> <secs> | throughput <proxy> <direct> [mb]"
        ),
    }
}
