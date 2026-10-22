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
            slug: None,
            avoid_ports: Vec::new(),
            modpack: None,
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

fn mod_jars(dir: &std::path::Path) -> Vec<String> {
    let mut jars: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .filter(|n| n.ends_with(".jar"))
        .collect();
    jars.sort();
    jars
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs Docker and downloads a modpack and a Minecraft server"]
async fn modpack_update_swaps_the_packs_files() {
    // "Essentially Optimized Server", a small Fabric pack with a version per
    // Minecraft release: 1.21.10 → 1.21.11 replaces most of its mods.
    const OLD: &str = "k2uQ6TzC";
    const NEW: &str = "D2dYG22N";
    let pack = |version: &str| nn_orchestrator::spec::Modpack {
        platform: "modrinth".into(),
        project_id: "sTicmJba".into(),
        slug: "essentially-optimized-server".into(),
        title: "Essentially Optimized Server".into(),
        version_id: Some(version.into()),
        version_name: None,
        loader: Some("fabric".into()),
        icon_url: None,
        page_url: None,
        ..Default::default()
    };
    let dir = tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap();
    let servers_dir = dir.path().join("servers");
    let store = Store::open(&dir.path().join("test.db")).await.unwrap();
    store.migrate().await.unwrap();
    let events = EventBus::default();
    let orch = orchestrator(&store, &events, &servers_dir).await;
    nn_orchestrator::spawn_background(&orch);

    println!("create + start (pack for 1.21.10)");
    let server = orch
        .create(CreateServer {
            name: "NN Modpack Test".into(),
            loader: "modrinth".into(),
            mc_version: "1.21.10".into(),
            loader_version: None,
            memory_mb: 2048,
            card_theme: "end".into(),
            eula_accepted: true,
            sleep_when_empty: false,
            start_after_create: true,
            slug: None,
            avoid_ports: Vec::new(),
            modpack: Some(pack(OLD)),
        })
        .await
        .unwrap();
    let id = server.id.clone();
    wait_for(&store, &id, "running", Duration::from_secs(900)).await;
    orch.stop(&id, false).await.unwrap();
    wait_for(&store, &id, "stopped", Duration::from_secs(60)).await;
    let mods = servers_dir.join(&server.slug).join("mods");
    let before = mod_jars(&mods);
    assert!(
        before.contains(&"fabric-api-0.138.3+1.21.10.jar".to_owned()),
        "{before:?}"
    );
    // Something the pack didn't install.
    std::fs::write(mods.join("keep-me.txt"), b"mine").unwrap();

    println!("update to the pack for 1.21.11 and start");
    let mut settings = store.find_server(&id).await.unwrap().unwrap().settings();
    settings.modpack_json = Some(serde_json::to_string(&pack(NEW)).unwrap());
    settings.mc_version = "1.21.11".into();
    orch.update(&id, settings).await.unwrap();
    orch.start(&id).await.unwrap();
    wait_for(&store, &id, "running", Duration::from_secs(900)).await;
    let after = mod_jars(&mods);
    println!("  mods before: {before:?}\n  mods after: {after:?}");
    assert!(
        after.contains(&"fabric-api-0.141.1+1.21.11.jar".to_owned()),
        "new version's mods installed: {after:?}"
    );
    assert!(
        !after.contains(&"fabric-api-0.138.3+1.21.10.jar".to_owned()),
        "old version's mods removed: {after:?}"
    );
    assert!(
        mods.join("keep-me.txt").exists(),
        "files the pack didn't install are left alone"
    );

    orch.stop(&id, false).await.unwrap();
    wait_for(&store, &id, "stopped", Duration::from_secs(60)).await;
    orch.delete(&id, true).await.unwrap();
}

/// An HTTP GET of `/` on 127.0.0.1:`port`: the status code, if it answered.
async fn http_status(port: u16) -> Option<u16> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .ok()?;
    stream
        .write_all(b"GET / HTTP/1.0\r\nHost: localhost\r\n\r\n")
        .await
        .ok()?;
    let mut head = vec![0u8; 64];
    let n = stream.read(&mut head).await.ok()?;
    let line = String::from_utf8_lossy(&head[..n]).to_string();
    line.split_whitespace().nth(1)?.parse().ok()
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs Docker and downloads a Minecraft server and BlueMap"]
async fn bluemap_module_runs_next_to_its_server() {
    use nn_orchestrator::modules::{self, Lifecycle};
    let dir = tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap();
    let servers_dir = dir.path().join("servers");
    let store = Store::open(&dir.path().join("test.db")).await.unwrap();
    store.migrate().await.unwrap();
    let events = EventBus::default();
    let orch = orchestrator(&store, &events, &servers_dir).await;
    nn_orchestrator::spawn_background(&orch);

    println!("create + start a vanilla server so its world exists");
    let server = orch
        .create(CreateServer {
            name: "NN Module Test".into(),
            loader: "vanilla".into(),
            mc_version: "1.21.4".into(),
            loader_version: None,
            memory_mb: 1024,
            card_theme: "end".into(),
            eula_accepted: true,
            sleep_when_empty: false,
            start_after_create: true,
            slug: None,
            avoid_ports: Vec::new(),
            modpack: None,
        })
        .await
        .unwrap();
    let id = server.id.clone();
    wait_for(&store, &id, "running", Duration::from_secs(600)).await;
    orch.stop(&id, false).await.unwrap();
    wait_for(&store, &id, "stopped", Duration::from_secs(60)).await;

    println!("add BlueMap (started by hand) and start it");
    let settings = serde_json::json!({ "accept_download": true, "render_threads": 1 });
    let module = orch
        .add_module(
            &id,
            "bluemap",
            None,
            settings.as_object().unwrap(),
            Some(Lifecycle::Manual),
            Some(1536),
        )
        .await
        .unwrap();
    let port = u16::try_from(module.web_port.expect("BlueMap has a web page")).unwrap();
    orch.start_module(&module.id).await.unwrap();
    let started = Instant::now();
    let answered = loop {
        if let Some(code) = http_status(port).await {
            break code;
        }
        let state = &orch.modules(&id).await.unwrap()[0].1;
        assert_ne!(
            state.status,
            "failed",
            "BlueMap stopped: {:?}\n{}",
            state.message,
            orch.module_logs(&module.id, 80).await.unwrap().join("\n")
        );
        assert!(
            started.elapsed() < Duration::from_secs(600),
            "BlueMap's web page never answered:\n{}",
            orch.module_logs(&module.id, 80).await.unwrap().join("\n")
        );
        tokio::time::sleep(Duration::from_secs(3)).await;
    };
    println!(
        "  → web page answered {answered} after {:.0?}",
        started.elapsed()
    );
    assert_eq!(answered, 200);
    let slug = &server.slug;
    let module_dir = servers_dir.join(slug).join("modules").join("bluemap");
    let core = std::fs::read_to_string(module_dir.join("config/core.conf")).unwrap();
    assert!(core.contains("accept-download: true"), "{core}");
    assert!(
        module_dir.join("cache/web").exists(),
        "rendered output goes in the cache"
    );

    println!("the server joins the modules' network when it starts");
    orch.start(&id).await.unwrap();
    wait_for(&store, &id, "running", Duration::from_secs(300)).await;
    let docker =
        bollard::Docker::connect_with_socket(SOCKET, 30, bollard::API_DEFAULT_VERSION).unwrap();
    let network = docker
        .inspect_network(&modules::network_name(slug), None)
        .await
        .expect("the modules' network exists");
    let members: Vec<String> = network
        .containers
        .unwrap_or_default()
        .values()
        .filter_map(|c| c.name.clone())
        .collect();
    assert!(
        members.contains(&nn_orchestrator::spec::container_name(slug)),
        "{members:?}"
    );
    assert!(
        members.contains(&modules::container_name(slug, "bluemap")),
        "{members:?}"
    );

    println!("stop the module, then delete the server with everything");
    orch.stop_module(&module.id).await.unwrap();
    assert_eq!(orch.modules(&id).await.unwrap()[0].1.status, "stopped");
    orch.stop(&id, false).await.unwrap();
    wait_for(&store, &id, "stopped", Duration::from_secs(60)).await;
    orch.delete(&id, true).await.unwrap();
    assert!(
        docker
            .inspect_container(&modules::container_name(slug, "bluemap"), None)
            .await
            .is_err(),
        "the module's container is gone"
    );
    assert!(
        docker
            .inspect_network(&modules::network_name(slug), None)
            .await
            .is_err(),
        "the network is gone"
    );
}
