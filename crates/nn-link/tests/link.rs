//! Panel and agent over an in-memory pipe.

use std::time::Duration;

use bytes::BytesMut;
use futures_util::StreamExt;
use nn_link::messages::{self, AgentMessage, Hello, HostReport, JoinProof, Welcome};
use nn_link::{Frames, JoinToken, Keypair, LinkError, PROTOCOL, accept, connect};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_util::codec::{Framed, LengthDelimitedCodec};

fn pipe() -> (Frames, Frames) {
    let (a, b) = tokio::io::duplex(1 << 20);
    let frames = |io| -> Frames {
        let codec = LengthDelimitedCodec::builder()
            .max_frame_length(70_000)
            .new_codec();
        Box::new(Framed::new(io, codec).map(|r| r.map(BytesMut::freeze)))
    };
    (frames(a), frames(b))
}

fn hello(join: Option<JoinProof>) -> Hello {
    Hello {
        protocol: PROTOCOL,
        version: "0.1.0".into(),
        join,
    }
}

async fn within<T>(f: impl std::future::Future<Output = T>) -> T {
    tokio::time::timeout(Duration::from_secs(10), f)
        .await
        .expect("timed out")
}

#[tokio::test]
async fn an_agent_joins_and_streams_flow_both_ways() {
    let (panel_frames, agent_frames) = pipe();
    let (panel_key, agent_key) = (Keypair::generate(), Keypair::generate());
    let token = JoinToken::generate(panel_key.public);
    let proof = JoinProof {
        code_id: token.code_id.clone(),
        secret: token.secret_hex(),
    };

    let (expected_agent, stored_hash) = (agent_key.public, token.secret_hash());
    let panel = tokio::spawn(async move {
        let shaken = accept(panel_frames, &panel_key).await.unwrap();
        assert_eq!(shaken.agent_key, expected_agent);
        let join = shaken.hello.join.clone().expect("a join proof");
        assert_eq!(nn_link::hash_secret(&join.secret), stored_hash);
        let (session, mut inbound) = shaken.admit("n1", "Basement PC").await.unwrap();
        let mut control = inbound.next().await.expect("control stream");
        let message: AgentMessage = messages::recv(&mut control).await.unwrap().unwrap();
        // A big reply crosses many Noise messages.
        control.write_all(&vec![7u8; 1_000_000]).await.unwrap();
        control.flush().await.unwrap();
        (session, message)
    });

    let (welcome, session, _inbound) = within(connect(
        agent_frames,
        &agent_key,
        &token.panel_key,
        &hello(Some(proof)),
    ))
    .await
    .unwrap();
    assert_eq!(
        welcome,
        Welcome::Accepted {
            node_id: "n1".into(),
            name: "Basement PC".into()
        }
    );
    let mut control = session.open().await.unwrap();
    let heartbeat = AgentMessage::Heartbeat {
        host: HostReport {
            hostname: "nn-node2".into(),
            cpus: 2,
            ..Default::default()
        },
    };
    messages::send(&mut control, &heartbeat).await.unwrap();
    let mut big = vec![0u8; 1_000_000];
    within(control.read_exact(&mut big)).await.unwrap();
    assert!(big.iter().all(|b| *b == 7));

    let (panel_session, received) = within(panel).await.unwrap();
    assert_eq!(received, heartbeat);

    // Closing on one side ends the link on the other.
    panel_session.close();
    within(session.closed()).await;
    assert!(session.open().await.is_err());
}

#[tokio::test]
async fn an_agent_refuses_a_panel_with_another_key() {
    let (panel_frames, agent_frames) = pipe();
    let (panel_key, agent_key) = (Keypair::generate(), Keypair::generate());
    let impostor_of = Keypair::generate().public;
    let panel = tokio::spawn(async move { accept(panel_frames, &panel_key).await.map(|_| ()) });
    let result = within(connect(
        agent_frames,
        &agent_key,
        &impostor_of,
        &hello(None),
    ))
    .await;
    assert!(
        matches!(result, Err(LinkError::WrongPanel)),
        "{:?}",
        result.err()
    );
    // The panel never hears a Hello.
    assert!(within(panel).await.unwrap().is_err());
}

#[tokio::test]
async fn a_rejected_agent_is_told_why() {
    let (panel_frames, agent_frames) = pipe();
    let (panel_key, agent_key) = (Keypair::generate(), Keypair::generate());
    let pinned = panel_key.public;
    tokio::spawn(async move {
        let shaken = accept(panel_frames, &panel_key).await.unwrap();
        assert!(shaken.hello.join.is_none());
        shaken.reject("this node was removed").await;
    });
    let result = within(connect(agent_frames, &agent_key, &pinned, &hello(None))).await;
    match result {
        Err(LinkError::Rejected(reason)) => assert_eq!(reason, "this node was removed"),
        other => panic!("expected a rejection, got {:?}", other.err()),
    }
}

#[tokio::test]
async fn garbage_instead_of_a_handshake_fails_cleanly() {
    use futures_util::SinkExt;
    let (panel_frames, mut attacker) = pipe();
    let panel_key = Keypair::generate();
    let panel = tokio::spawn(async move { accept(panel_frames, &panel_key).await.map(|_| ()) });
    attacker
        .send(bytes::Bytes::from_static(b"definitely not noise"))
        .await
        .unwrap();
    drop(attacker);
    assert!(within(panel).await.unwrap().is_err());
}
