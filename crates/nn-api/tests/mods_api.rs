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
use nn_api::{ApiConfig, AppState, ProxyRuntime, StoreProxyHooks};
use nn_auth::{Auth, AuthConfig};
use nn_events::EventBus;
use nn_node::{LocalNode, Nodes};
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
    harness_with(Default::default()).await
}

async fn harness_with(content: Arc<nn_api::content::Content>) -> Harness {
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
        events: events.clone(),
        config: Arc::new(ApiConfig::default()),
        metrics: None,
        shutdown: shutdown_rx,
        players: Default::default(),
        uploads: Default::default(),
        content,
        notifier: Default::default(),
        install_count: Default::default(),
        agents: Default::default(),
        nodes: Nodes::new(
            LocalNode::without_docker("not connected", Some(servers_dir.clone()), events.clone()),
            Some(store.clone()),
        ),
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

/// A Modrinth stand-in with one pack, PACK, in four versions: v1 and v2 for
/// 1.21.1, v3 for 1.21.4, and v4 (newest) for 1.20.1.
async fn fake_modrinth() -> String {
    let version = |id: &str, month: u32, mc: &str| {
        json!({
            "id": id, "project_id": "PACK", "name": format!("Pack {id}"), "version_number": id,
            "game_versions": [mc], "loaders": ["fabric"], "version_type": "release",
            "date_published": format!("2026-{month:02}-01T00:00:00Z"), "files": [],
        })
    };
    let versions = Arc::new(vec![
        version("v4", 4, "1.20.1"),
        version("v3", 3, "1.21.4"),
        version("v2", 2, "1.21.1"),
        version("v1", 1, "1.21.1"),
    ]);
    let all = versions.clone();
    let project = json!({
        "id": "PACK", "slug": "pack", "title": "Pack", "description": "A pack", "icon_url": null,
        "downloads": 1, "loaders": ["fabric"], "game_versions": ["1.21.1", "1.21.4"],
        "project_type": "modpack", "server_side": "required", "updated": "2026-04-01T00:00:00Z",
    });
    let app = Router::new()
        .route(
            "/v2/project/{id}",
            axum::routing::get(move || {
                let project = project.clone();
                async move { axum::Json(project) }
            }),
        )
        .route(
            "/v2/project/{id}/version",
            axum::routing::get(move || {
                let all = all.clone();
                async move { axum::Json(Value::Array(all.to_vec())) }
            }),
        )
        .route(
            "/v2/version/{id}",
            axum::routing::get(
                move |axum::extract::Path(id): axum::extract::Path<String>| {
                    let versions = versions.clone();
                    async move {
                        match versions.iter().find(|v| v["id"] == id.as_str()) {
                            Some(v) => Ok(axum::Json(v.clone())),
                            None => Err(StatusCode::NOT_FOUND),
                        }
                    }
                },
            ),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    format!("http://{addr}")
}

#[tokio::test]
async fn modpack_updates_list_and_refuse() {
    let modrinth = fake_modrinth().await;
    let catalog = nn_catalog::Catalog::new(
        None,
        nn_catalog::Endpoints {
            modrinth,
            ..Default::default()
        },
    );
    let h = harness_with(Arc::new(nn_api::content::Content::new(catalog, None, None))).await;
    let id = h.server("mrpack", "modrinth").await;
    let mut settings = h.store.find_server(&id).await.unwrap().unwrap().settings();
    settings.mc_version = "1.21.1".into();
    settings.modpack_json = Some(
        json!({ "platform": "modrinth", "project_id": "PACK", "slug": "pack", "title": "Pack", "version_id": "v2", "version_name": "v2", "loader": "fabric", "icon_url": null, "page_url": "https://modrinth.com/modpack/pack" })
            .to_string(),
    );
    assert!(
        h.store
            .update_server_settings(&id, &settings)
            .await
            .unwrap()
    );

    let (status, body) = h
        .call("CheckModpackUpdate", json!({ "serverId": id }))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["modpack"]["title"], "Pack");
    assert_eq!(body["current"]["id"], "v2");
    let newer: Vec<&str> = body["newer"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["id"].as_str().unwrap())
        .collect();
    assert_eq!(newer, ["v3"], "not v1 (older) or v4 (older Minecraft)");

    for (version, says) in [
        ("v2", "already installed"),
        ("v1", "isn't newer"),
        ("v4", "older Minecraft"),
        ("nope", "no such item"),
    ] {
        let (status, body) = h
            .call(
                "UpdateModpack",
                json!({ "serverId": id, "versionId": version }),
            )
            .await;
        assert_ne!(status, StatusCode::OK, "{version}: {body}");
        let message = body["message"].as_str().unwrap_or_default();
        assert!(message.contains(says), "{version}: {message}");
    }
    // A good version, but nothing can run without Docker: refused before
    // anything is stopped or backed up.
    let (status, body) = h
        .call(
            "UpdateModpack",
            json!({ "serverId": id, "versionId": "v3" }),
        )
        .await;
    assert_ne!(status, StatusCode::OK, "{body}");
    assert_eq!(body["code"], "failed_precondition", "{body}");
    assert!(h.store.list_backups(Some(&id)).await.unwrap().is_empty());
    let server = h.store.find_server(&id).await.unwrap().unwrap();
    assert!(server.modpack_json.unwrap().contains("\"v2\""));

    // Not a modpack.
    let plain = h.server("plainfabric", "fabric").await;
    let (_, body) = h
        .call("CheckModpackUpdate", json!({ "serverId": plain }))
        .await;
    assert_eq!(body["code"], "failed_precondition", "{body}");
}
