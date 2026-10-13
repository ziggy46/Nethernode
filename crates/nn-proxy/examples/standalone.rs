//! Runs the proxy alone, for benchmarks: every hostname routes to one backend.
//!
//! `cargo run --release -p nn-proxy --example standalone -- <listen> <backend>`
//! e.g. `-- 127.0.0.1:25600 127.0.0.1:25601`

use std::sync::Arc;

use nn_events::EventBus;
use nn_proxy::{
    BackendSpec, Limits, ListenerSpec, NoopHooks, Proxy, ProxyConfig, RouteSpec, ServerState,
};

#[tokio::main]
async fn main() {
    let mut args = std::env::args().skip(1);
    let listen = args
        .next()
        .unwrap_or_else(|| "127.0.0.1:25600".into())
        .parse()
        .expect("listen addr");
    let backend = args
        .next()
        .unwrap_or_else(|| "127.0.0.1:25601".into())
        .parse()
        .expect("backend addr");

    // A load test opens thousands of connections from one IP.
    let limits = Limits {
        per_ip_burst: 1_000_000,
        per_ip_per_second: 1_000_000.0,
        ..Limits::default()
    };
    let proxy = Proxy::new(Arc::new(NoopHooks), EventBus::default(), limits);
    let config = ProxyConfig {
        listeners: vec![ListenerSpec {
            id: "bench".into(),
            bind: listen,
            proxy_protocol: false,
        }],
        routes: vec![RouteSpec {
            id: "all".into(),
            pattern: "*".into(),
            server_id: Some("echo".into()),
            listener_id: None,
            priority: 0,
        }],
        backends: vec![BackendSpec {
            server_id: "echo".into(),
            name: "echo".into(),
            slug: "echo".into(),
            addr: backend,
            state: ServerState::Running,
        }],
    };
    println!("{:?}", proxy.apply(config).await);
    println!("pid {}", std::process::id());
    tokio::signal::ctrl_c().await.ok();
}
