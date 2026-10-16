//! Per-connection CPU cost before splicing: parse the handshake, normalize
//! the hostname, and resolve a route in a table of 1,000 servers.

use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
use nn_mcproto::codec::frame;
use nn_mcproto::{Handshake, NextState, parse_frame};
use nn_proxy::{BackendSpec, RouteSpec, RouteTable, ServerState};

fn table(n: usize) -> RouteTable {
    let backends: Vec<_> = (0..n)
        .map(|i| BackendSpec {
            remote: false,
            server_id: format!("s{i}"),
            name: format!("Server {i}"),
            slug: format!("s{i}"),
            addr: "127.0.0.1:30000".parse().unwrap(),
            state: ServerState::Running,
        })
        .collect();
    let mut routes: Vec<_> = (0..n)
        .map(|i| RouteSpec {
            id: format!("r{i}"),
            pattern: format!("s{i}.example.com"),
            server_id: Some(format!("s{i}")),
            listener_id: None,
            priority: 0,
        })
        .collect();
    routes.push(RouteSpec {
        id: "w".into(),
        pattern: "*.mc.example.com".into(),
        server_id: None,
        listener_id: None,
        priority: 0,
    });
    RouteTable::build(&routes, &backends, &[])
}

fn bench(c: &mut Criterion) {
    let table = table(1_000);
    let hs = Handshake {
        protocol_version: 777,
        server_address: "S500.Example.com.\0FML3\0".into(),
        server_port: 25565,
        next_state: NextState::Login,
    };
    let bytes = frame(&hs.encode());

    c.bench_function("parse + normalize + resolve (exact)", |b| {
        b.iter(|| {
            let f = parse_frame(black_box(&bytes), 2048).unwrap().unwrap();
            let hs = Handshake::decode(&bytes[f.payload]).unwrap();
            black_box(table.resolve("main", &hs.hostname()))
        })
    });
    c.bench_function("resolve slug wildcard", |b| {
        b.iter(|| black_box(table.resolve("main", black_box("s999.mc.example.com"))))
    });
}

criterion_group!(benches, bench);
criterion_main!(benches);
