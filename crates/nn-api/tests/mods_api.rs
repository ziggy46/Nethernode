//! ModService against real mod folders: listing jars with their metadata,
//! turning mods off and on, deleting, icons, manual CurseForge downloads,
//! and refusing what isn't a mod.

use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use http::{Request, StatusCode, header};
use http_body_util::BodyExt;
use nn_api::{ApiConfig, AppState, ProxyRuntime, ServerManager, StoreProxyHooks};
use nn_auth::{Auth, AuthConfig};
use nn_events::EventBus;
use nn_store::{NewServer, Store};
use serde_json::{Value, json};
use tower::ServiceExt;

const CSRF: &str = "csrf";

struct Harness {
    app: Router,
    store: Store,
    owner: String,
    servers_dir: PathBuf,
    _tmp: tempfile::TempDir,
}

fn jar(files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(&mut out));
        for (name, data) in files {
            zip.start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(data).unwrap();
        }
        zip.finish().unwrap();
    }
    out
}

async fn harness() -> Harness {
    let tmp = tempfile::tempdir().unwrap();
    let servers_dir = tmp.path().canonicalize().unwrap().join("servers");
    let store = Store::open_in_memory().await.unwrap();
    store.migrate().await.unwrap();
    let events = EventBus::default();
    let proxy = nn_proxy::Proxy::new(
        StoreProxyHooks::new(store.clone()),
        events.clone(),
        nn_proxy::Limits::default(),
    );
    let auth = Arc::new(Auth::new(store.clone(), AuthConfig::default()));
    let (_shutdown, shutdown_rx) = tokio::sync::watch::channel(false);
    let app = nn_api::router(AppState {
        proxy: Arc::new(ProxyRuntime::new(proxy, store.clone(), true)),
        auth: auth.clone(),
        events,
        config: Arc::new(ApiConfig::default()),
        metrics: None,
        shutdown: shutdown_rx,
        players: Default::default(),
        uploads: Default::default(),
        content: Default::default(),
        notifier: Default::default(),
        install_count: Default::default(),
        servers: ServerManager {
            servers_dir: Some(servers_dir.clone()),
            ..Default::default()
        },
    });
    let owner = auth
        .setup("owner", "correct horse battery", Default::default())
        .await
        .unwrap()
        .token;
    Harness {
        app,
        store,
        owner,
        servers_dir,
        _tmp: tmp,
    }
}

impl Harness {
    async fn server(&self, slug: &str, loader: &str) -> String {
        let id = self
            .store
            .insert_server(NewServer {
                name: slug,
                slug,
                status: "stopped",
                mc_version: "1.20.1",
                loader,
                image: "itzg/minecraft-server",
                memory_mb: 2048,
                internal_port: 30_500 + slug.len() as i64,
                sleep_when_empty: false,
            })
            .await
            .unwrap()
            .id;
        std::fs::create_dir_all(self.servers_dir.join(slug).join("mods")).unwrap();
        id
    }

    async fn call(&self, method: &str, body: Value) -> (StatusCode, Value) {
        let req = Request::post(format!("/api/rpc/nethernode.v1.ModService/{method}"))
            .header(header::CONTENT_TYPE, "application/json")
            .header(
                header::COOKIE,
                format!("nn_csrf={CSRF}; nn_session={}", self.owner),
            )
            .header("x-csrf-token", CSRF)
            .body(Body::from(body.to_string()))
            .unwrap();
        let response = self.app.clone().oneshot(req).await.unwrap();
        let status = response.status();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }
}

#[tokio::test]
async fn lists_toggles_and_deletes_mods() {
    let h = harness().await;
    let id = h.server("modded", "fabric").await;
    let mods = h.servers_dir.join("modded/mods");
    std::fs::write(
        mods.join("sodium-0.6.jar"),
        jar(&[
            (
                "fabric.mod.json",
                br#"{"id":"sodium","name":"Sodium","version":"0.6.0","icon":"icon.png","authors":["JellySquid"]}"#,
            ),
            ("icon.png", b"\x89PNG\r\n\x1a\nfake"),
        ]),
    )
    .unwrap();
    std::fs::write(mods.join("broken.jar"), b"not a zip").unwrap();
    std::fs::write(mods.join("notes.txt"), b"not a mod").unwrap();

    let (status, body) = h.call("ListMods", json!({ "serverId": id })).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["supported"], true);
    assert_eq!(body["loader"], "fabric");
    let list = body["mods"].as_array().unwrap();
    assert_eq!(list.len(), 2, "jars only: {body}");
    let sodium = list
        .iter()
        .find(|m| m["filename"] == "sodium-0.6.jar")
        .unwrap();
    assert_eq!(sodium["name"], "Sodium");
    assert_eq!(sodium["version"], "0.6.0");
    assert_eq!(sodium["enabled"], true);
    assert_eq!(sodium["hasIcon"], true);

    let req = Request::get(format!("/api/servers/{id}/mods/icon?file=sodium-0.6.jar"))
        .header(header::COOKIE, format!("nn_session={}", h.owner))
        .body(Body::empty())
        .unwrap();
    let icon = h.app.clone().oneshot(req).await.unwrap();
    assert_eq!(icon.status(), StatusCode::OK);
    assert_eq!(icon.headers()[header::CONTENT_TYPE], "image/png");

    let (status, body) = h
        .call(
            "SetModEnabled",
            json!({ "serverId": id, "filename": "sodium-0.6.jar", "enabled": false }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["filename"], "sodium-0.6.jar.disabled");
    assert!(mods.join("sodium-0.6.jar.disabled").exists());
    let (_, body) = h.call("ListMods", json!({ "serverId": id })).await;
    assert!(
        body["mods"]
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m["filename"] == "sodium-0.6.jar.disabled" && m["enabled"] != true)
    );
    h.call(
        "SetModEnabled",
        json!({ "serverId": id, "filename": "sodium-0.6.jar.disabled", "enabled": true }),
    )
    .await;
    assert!(mods.join("sodium-0.6.jar").exists());

    let (status, _) = h
        .call(
            "DeleteMod",
            json!({ "serverId": id, "filename": "broken.jar" }),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert!(!mods.join("broken.jar").exists(), "moved to the trash");
}

#[tokio::test]
async fn refuses_what_isnt_a_mod() {
    let h = harness().await;
    let id = h.server("modded", "forge").await;
    std::fs::write(h.servers_dir.join("modded/server.properties"), b"x").unwrap();
    for name in [
        "../server.properties",
        "a/b.jar",
        "server.properties",
        "../../x.jar",
    ] {
        let (status, body) = h
            .call("DeleteMod", json!({ "serverId": id, "filename": name }))
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{name}: {body}");
    }
    assert!(h.servers_dir.join("modded/server.properties").exists());

    let paper = h.server("plugins", "paper").await;
    let (status, body) = h.call("ListMods", json!({ "serverId": paper })).await;
    assert_eq!(status, StatusCode::OK);
    assert_ne!(body["supported"], true, "false is left out of the JSON");
    let (status, _) = h
        .call(
            "SetModEnabled",
            json!({ "serverId": paper, "filename": "x.jar", "enabled": false }),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "no mods on Paper");
}

#[tokio::test]
async fn curseforge_packs_list_files_to_download_by_hand() {
    let h = harness().await;
    let id = h.server("cfpack", "curseforge").await;
    sqlx_set_modpack(&h.store, &id).await;
    let table = "\
Mod               Version Name  Filename          Download page
================  ============  ================  =====================================================
Restricted Mod    restricted-1  restricted-1.jar  https://www.curseforge.com/minecraft/mc-mods/r/download/1
Another One       another-2     another-2.jar     https://www.curseforge.com/minecraft/mc-mods/a/download/2
";
    let root = h.servers_dir.join("cfpack");
    std::fs::write(root.join("MODS_NEED_DOWNLOAD.txt"), table).unwrap();
    std::fs::create_dir_all(root.join("downloads/mods")).unwrap();
    std::fs::write(root.join("downloads/mods/another-2.jar"), b"jar").unwrap();
    let (status, body) = h.call("ListMods", json!({ "serverId": id })).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["loader"], "forge");
    assert_eq!(body["downloadsFolder"], "downloads/mods");
    let manual = body["manualDownloads"].as_array().unwrap();
    assert_eq!(manual.len(), 2);
    assert_eq!(manual[0]["filename"], "restricted-1.jar");
    assert_eq!(
        manual[0]["present"],
        Value::Null,
        "not uploaded yet (false is omitted)"
    );
    assert_eq!(manual[1]["present"], true);
}

async fn sqlx_set_modpack(store: &Store, id: &str) {
    let mut server = store.find_server(id).await.unwrap().unwrap().settings();
    server.modpack_json = Some(
        json!({ "platform": "curseforge", "project_id": "", "slug": "pack", "title": "Pack", "version_id": null, "version_name": null, "loader": "forge", "icon_url": null, "page_url": null })
            .to_string(),
    );
    assert!(store.update_server_settings(id, &server).await.unwrap());
}
