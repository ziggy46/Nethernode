//! Arbitrary bytes on a link stream, read as each message type the panel
//! and agents exchange: decoding must fail cleanly or round-trip. Run:
//! `cargo +nightly fuzz run messages` from `crates/nn-link`.
#![no_main]

use libfuzzer_sys::fuzz_target;
use nn_link::messages::{self, AgentMessage, Hello, StreamKind, Welcome};

async fn read_all<T: serde::de::DeserializeOwned + serde::Serialize>(data: &[u8]) {
    let mut input = data;
    while let Ok(Some(message)) = messages::recv::<_, T>(&mut input).await {
        // What decoded encodes again, within the size limit it came in under.
        let mut out = Vec::new();
        messages::send(&mut out, &message).await.unwrap();
        assert!(out.len() <= messages::MAX_MESSAGE + 4);
    }
}

fuzz_target!(|data: &[u8]| {
    let runtime = tokio::runtime::Builder::new_current_thread().build().unwrap();
    runtime.block_on(async {
        read_all::<Hello>(data).await;
        read_all::<Welcome>(data).await;
        read_all::<AgentMessage>(data).await;
        read_all::<StreamKind>(data).await;
    });
});
