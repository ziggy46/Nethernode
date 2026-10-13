//! Real Docker end-to-end test. Slow (pulls an image, boots a Minecraft
//! server), so it's ignored by default:
//!
//! `cargo test -p nn-orchestrator --test docker -- --ignored --nocapture`

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use nn_auth::SecretBox;
use nn_events::{Event, EventBus};
use nn_orchestrator::{CreateServer, Orchestrator, OrchestratorConfig};
use nn_store::Store;

const SOCKET: &str = "unix:///var/run/docker.sock";

async fn orchestrator(
    store: &Store,
    events: &EventBus,
    servers_dir: &std::path::Path,
) -> Arc<Orchestrator> {
    let secrets = Arc::new(SecretBox::from_hex_key(&"42".repeat(32)).unwrap());
    let mut config = OrchestratorConfig::new(servers_dir.to_path_buf());
    // Stay clear of anything else on the default range.
    config.port_range = 31500..=31599;
    config.player_poll = Duration::from_secs(5);
    Orchestrator::connect(SOCKET, store.clone(), events.clone(), secrets, config)
        .await
        .expect("Docker reachable")
}

async fn wait_for(store: &Store, id: &str, status: &str, limit: Duration) {
    let started = Instant::now();
    loop {
        let current = store
            .find_server(id)
            .await
            .unwrap()
            .map(|s| s.status)
            .unwrap_or_default();
        if current == status {
            println!("  → {status} after {:.0?}", started.elapsed());
            return;
        }
        assert!(
            started.elapsed() < limit,
            "timed out waiting for {status}; still {current}"
        );
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

async fn container_started_at(id: &str) -> String {
    let docker =
        bollard::Docker::connect_with_socket(SOCKET, 30, bollard::API_DEFAULT_VERSION).unwrap();
    docker
        .inspect_container(id, None)
        .await
        .unwrap()
        .state
        .unwrap()
        .started_at
        .unwrap()
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs Docker and downloads a Minecraft server"]
async fn create_start_adopt_sleep_wake_stop_delete() {
    let dir = tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap();
    let servers_dir = dir.path().join("servers");
    let store = Store::open(&dir.path().join("test.db")).await.unwrap();
    store.migrate().await.unwrap();
    let events = EventBus::default();
    let mut rx = events.subscribe();
    let orch = orchestrator(&store, &events, &servers_dir).await;
    nn_orchestrator::spawn_background(&orch);

    println!("create + start (vanilla 1.21.4)");
    let server = orch
        .create(CreateServer {
            name: "NN Integration Test".into(),
            loader: "vanilla".into(),
            mc_version: "1.21.4".into(),
            loader_version: None,
            memory_mb: 1024,
            card_theme: "end".into(),
            eula_accepted: true,
            sleep_when_empty: true,
            start_after_create: true,
        })
        .await
        .unwrap();
    assert_eq!(server.status, "installing");
    assert_eq!(server.image, "itzg/minecraft-server:java21");
    let id = server.id.clone();
    wait_for(&store, &id, "running", Duration::from_secs(600)).await;

    let server = store.find_server(&id).await.unwrap().unwrap();
    let addr = SocketAddr::from(([127, 0, 0, 1], u16::try_from(server.internal_port).unwrap()));
    let status = nn_mcproto::client::ping(addr, &[], Duration::from_secs(5))
        .await
        .expect("server answers pings");
    println!("  ping: {} — {:?}", status.version.name, status.description);
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert!(!orch.stats_history(&id).is_empty(), "stats are sampled");
    let (logs, _) = orch.subscribe_logs(&id);
    assert!(
        logs.iter().any(|l| l.contains("Done (")),
        "console output captured"
    );
    {
        // The server writes its files as NetherNode's user, so NetherNode
        // can manage them later.
        use std::os::unix::fs::MetadataExt;
        let ours = std::fs::metadata(&servers_dir).unwrap().uid();
        let props = servers_dir.join(&server.slug).join("server.properties");
        assert_eq!(std::fs::metadata(props).unwrap().uid(), ours);
    }
    {
        // Player management: a management port and the protocol's keys.
        assert!(server.mgmt_port.is_some(), "management port allocated");
        let props =
            std::fs::read_to_string(servers_dir.join(&server.slug).join("server.properties"))
                .unwrap();
        assert!(props.contains("management-server-enabled=true"));
        assert!(props.contains("management-server-tls-enabled=false"));
    }

    let mut saw_pull = false;
    while let Ok(event) = rx.try_recv() {
        saw_pull |= matches!(event, Event::ImagePullProgress { .. });
    }
    println!("  image pull progress events: {saw_pull} (false if the image was cached)");

    println!("simulate a NetherNode restart: new orchestrator adopts the running container");
    let container = server.container_id.clone().unwrap();
    let started_before = container_started_at(&container).await;
    drop(orch);
    let orch = orchestrator(&store, &events, &servers_dir).await;
    nn_orchestrator::spawn_background(&orch);
    tokio::time::sleep(Duration::from_secs(8)).await;
    wait_for(&store, &id, "running", Duration::from_secs(30)).await;
    assert_eq!(
        container_started_at(&container).await,
        started_before,
        "container was not restarted"
    );

    println!("sleep when empty (1 minute idle)");
    let mut settings = store.find_server(&id).await.unwrap().unwrap().settings();
    settings.auto_stop_idle_minutes = Some(1);
    let (_, restart) = orch.update(&id, settings).await.unwrap();
    assert!(!restart, "idle time doesn't need a container restart");
    wait_for(&store, &id, "sleeping", Duration::from_secs(150)).await;

    println!("wake on connect");
    orch.wake(&id).await.unwrap();
    wait_for(&store, &id, "running", Duration::from_secs(300)).await;

    println!("stop");
    orch.stop(&id, false).await.unwrap();
    wait_for(&store, &id, "stopped", Duration::from_secs(5)).await;

    println!("delete with data");
    orch.delete(&id, true).await.unwrap();
    assert!(store.find_server(&id).await.unwrap().is_none());
    assert!(!servers_dir.join(&server.slug).exists());
}
