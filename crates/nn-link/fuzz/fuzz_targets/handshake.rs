//! Arbitrary WebSocket frames where a handshake should be: what anyone who
//! can reach `/api/agent` (or sits between an agent and its panel) can
//! send. The panel side and the agent side must both fail cleanly, without
//! panicking or hanging. Run: `cargo +nightly fuzz run handshake` from
//! `crates/nn-link`.
#![no_main]

use std::time::Duration;

use bytes::{Bytes, BytesMut};
use futures_util::{SinkExt, StreamExt};
use libfuzzer_sys::fuzz_target;
use nn_link::messages::Hello;
use nn_link::{Frames, Keypair, PROTOCOL, accept, connect};
use tokio_util::codec::{Framed, LengthDelimitedCodec};

fn pipe() -> (Frames, Frames) {
    let (a, b) = tokio::io::duplex(1 << 20);
    let frames = |io| -> Frames {
        let codec = LengthDelimitedCodec::builder().max_frame_length(70_000).new_codec();
        Box::new(Framed::new(io, codec).map(|r| r.map(BytesMut::freeze)))
    };
    (frames(a), frames(b))
}

/// The input as frames: each starts with a length byte.
fn frames(mut data: &[u8]) -> Vec<Bytes> {
    let mut out = Vec::new();
    while let Some((&len, rest)) = data.split_first() {
        let take = usize::from(len).min(rest.len());
        out.push(Bytes::copy_from_slice(&rest[..take]));
        data = &rest[take..];
    }
    out
}

async fn feed(mut to: Frames, data: Vec<Bytes>) {
    for frame in data {
        if to.send(frame).await.is_err() {
            return;
        }
    }
}

fuzz_target!(|data: &[u8]| {
    let Some((&side, data)) = data.split_first() else { return };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .start_paused(true)
        .build()
        .unwrap();
    runtime.block_on(async {
        let (ours, theirs) = pipe();
        let attacker = tokio::spawn(feed(theirs, frames(data)));
        let key = Keypair::generate();
        let outcome = if side % 2 == 0 {
            // A stranger talking to the panel.
            tokio::time::timeout(Duration::from_secs(60), async { accept(ours, &key).await.map(|_| ()) }).await
        } else {
            // A stranger pretending to be the panel.
            let hello = Hello { protocol: PROTOCOL, version: "0.0.0".into(), join: None };
            let pinned = Keypair::generate().public;
            tokio::time::timeout(Duration::from_secs(60), async {
                connect(ours, &key, &pinned, &hello).await.map(|_| ())
            })
            .await
        };
        // Without the right keys, nothing gets through.
        assert!(!matches!(outcome, Ok(Ok(()))), "a handshake succeeded with garbage");
        attacker.abort();
    });
});
