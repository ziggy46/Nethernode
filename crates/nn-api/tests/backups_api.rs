//! Backups and schedules against a real server folder (no Docker, so the
//! server is stopped): back up, list, download, restore, retention-free
//! deletion, and schedule validation and previews.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use http::{Request, StatusCode, header};
use http_body_util::BodyExt;
use nn_api::{ApiConfig, AppState, ProxyRuntime, ServerManager, StoreProxyHooks};
use nn_auth::{Auth, AuthConfig};
use nn_events::{Event, EventBus};
use nn_store::{NewServer, Store};
use serde_json::{Value, json};
use tower::ServiceExt;

const CSRF: &str = "csrf";

struct Harness {
    app: Router,
    events: EventBus,
    owner: String,
    root: PathBuf,
    data: PathBuf,
    server_id: String,
    _tmp: tempfile::TempDir,
}

async fn harness() -> Harness {
    let tmp = tempfile::tempdir().unwrap();
    let data = tmp.path().canonicalize().unwrap();
    let servers_dir = data.join("servers");
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
        content: Default::default(),
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
    let server_id = store
        .insert_server(NewServer {
            name: "SMP",
            slug: "smp",
            status: "stopped",
            mc_version: "1.21.4",
            loader: "paper",
            image: "itzg/minecraft-server",
            memory_mb: 2048,
            internal_port: 30_500,
            sleep_when_empty: false,
        })
        .await
        .unwrap()
        .id;
    let root = servers_dir.join("smp");
    std::fs::create_dir_all(root.join("world/region")).unwrap();
    std::fs::create_dir_all(root.join("libraries")).unwrap();
    std::fs::write(root.join("world/level.dat"), b"original world").unwrap();
    std::fs::write(root.join("world/region/r.0.0.mca"), vec![3u8; 200_000]).unwrap();
    std::fs::write(root.join("server.properties"), b"motd=hi\n").unwrap();
    std::fs::write(root.join("libraries/big.jar"), b"downloaded again").unwrap();
    Harness {
        app,
        events,
        owner,
        root,
        data,
        server_id,
        _tmp: tmp,
    }
}

impl Harness {
    async fn call(&self, service: &str, method: &str, body: Value) -> (StatusCode, Value) {
        let req = Request::post(format!("/api/rpc/nethernode.v1.{service}/{method}"))
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

    async fn job(
        &self,
        rx: &mut tokio::sync::broadcast::Receiver<Event>,
        job: &str,
    ) -> (String, bool) {
        tokio::time::timeout(Duration::from_secs(20), async {
            loop {
                if let Ok(Event::FileJobFinished {
                    job_id,
                    message,
                    failed,
                    ..
                }) = rx.recv().await
                    && job_id == job
                {
                    return (message, failed);
                }
            }
        })
        .await
        .expect("job finished")
    }
}

#[tokio::test]
async fn back_up_download_and_restore() {
    let h = harness().await;
    let mut rx = h.events.subscribe();
    let (status, body) = h
        .call(
            "BackupService",
            "CreateBackup",
            json!({ "serverId": h.server_id }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (message, failed) = h.job(&mut rx, body["jobId"].as_str().unwrap()).await;
    assert!(!failed, "{message}");
    let backup_id = body["backupId"].as_str().unwrap().to_owned();

    let (_, list) = h
        .call(
            "BackupService",
            "ListBackups",
            json!({ "serverId": h.server_id }),
        )
        .await;
    let b = &list["backups"][0];
    assert_eq!(b["status"], "BACKUP_STATUS_READY");
    assert_eq!(b["kind"], "BACKUP_KIND_MANUAL");
    assert_eq!(b["files"], "3", "the libraries folder is left out: {b}");
    assert_eq!(b["createdBy"], "owner");
    assert!(std::fs::read_dir(h.data.join("backups")).unwrap().count() == 1);

    // Download, and resume part-way.
    let req = Request::get(format!("/api/backups/{backup_id}/download"))
        .header(header::COOKIE, format!("nn_session={}", h.owner))
        .header(header::RANGE, "bytes=10-")
        .body(Body::empty())
        .unwrap();
    let response = h.app.clone().oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
    let size: u64 = b["sizeBytes"].as_str().unwrap().parse().unwrap();
    let got = response.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(got.len() as u64, size - 10);
    let anon = Request::get(format!("/api/backups/{backup_id}/download"))
        .body(Body::empty())
        .unwrap();
    assert_eq!(
        h.app.clone().oneshot(anon).await.unwrap().status(),
        StatusCode::UNAUTHORIZED
    );

    // The world changes; restore puts it back and keeps what was there.
    std::fs::write(h.root.join("world/level.dat"), b"griefed").unwrap();
    std::fs::write(h.root.join("new-file.txt"), b"added later").unwrap();
    let (status, body) = h
        .call(
            "BackupService",
            "RestoreBackup",
            json!({ "backupId": backup_id }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (message, failed) = h.job(&mut rx, body["jobId"].as_str().unwrap()).await;
    assert!(!failed, "{message}");
    assert_eq!(
        std::fs::read(h.root.join("world/level.dat")).unwrap(),
        b"original world"
    );
    assert!(!h.root.join("new-file.txt").exists());
    assert_eq!(
        std::fs::read(h.root.join("libraries/big.jar")).unwrap(),
        b"downloaded again",
        "left-out folders carry over"
    );
    let leftovers: Vec<_> = std::fs::read_dir(h.root.parent().unwrap())
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    assert_eq!(leftovers, ["smp"], "no restoring/replaced folders left");
    let (_, list) = h.call("BackupService", "ListBackups", json!({})).await;
    let kinds: Vec<_> = list["backups"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| b["kind"].as_str().unwrap())
        .collect();
    assert!(kinds.contains(&"BACKUP_KIND_PRE_RESTORE"), "{kinds:?}");

    let (status, _) = h
        .call(
            "BackupService",
            "DeleteBackup",
            json!({ "backupId": backup_id }),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let (_, list) = h.call("BackupService", "ListBackups", json!({})).await;
    assert_eq!(list["backups"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn schedules_are_checked_and_previewed() {
    let h = harness().await;
    let schedule = |cron: &str, tz: &str, action: &str, text: &str| json!({ "schedule": { "serverId": h.server_id, "name": "Nightly", "cron": cron, "timezone": tz, "action": action, "text": text, "warnMinutes": [1, 5, 99], "keepDaily": 7, "keepWeekly": 4, "enabled": true } });
    let (status, body) = h
        .call(
            "ScheduleService",
            "UpsertSchedule",
            schedule(
                "0 4 * * *",
                "America/New_York",
                "SCHEDULE_ACTION_RESTART",
                "",
            ),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let s = &body["schedule"];
    assert_eq!(
        s["warnMinutes"],
        json!([5, 1]),
        "sorted, out-of-range dropped"
    );
    assert!(s["keepDaily"].is_null(), "retention only for backups");
    assert!(s["nextRunMs"].as_str().unwrap().parse::<i64>().unwrap() > 0);

    for (cron, tz, action, text, why) in [
        ("nope", "UTC", "SCHEDULE_ACTION_RESTART", "", "bad cron"),
        (
            "0 4 * * *",
            "Mars/Base",
            "SCHEDULE_ACTION_RESTART",
            "",
            "bad zone",
        ),
        (
            "0 4 * * *",
            "UTC",
            "SCHEDULE_ACTION_COMMAND",
            "",
            "no command",
        ),
        (
            "0 4 * * *",
            "UTC",
            "SCHEDULE_ACTION_UNSPECIFIED",
            "",
            "no action",
        ),
    ] {
        let (status, body) = h
            .call(
                "ScheduleService",
                "UpsertSchedule",
                schedule(cron, tz, action, text),
            )
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{why}: {body}");
    }

    let (_, preview) = h
        .call(
            "ScheduleService",
            "PreviewSchedule",
            json!({ "cron": "0 4 * * *", "timezone": "UTC" }),
        )
        .await;
    assert_eq!(preview["nextRunsMs"].as_array().unwrap().len(), 5);
    assert!(
        preview["description"].as_str().unwrap().contains("04:00"),
        "{preview}"
    );
    let (_, bad) = h
        .call(
            "ScheduleService",
            "PreviewSchedule",
            json!({ "cron": "x", "timezone": "UTC" }),
        )
        .await;
    assert!(bad["error"].as_str().unwrap().contains("not a valid"));

    let (_, list) = h
        .call(
            "ScheduleService",
            "ListSchedules",
            json!({ "serverId": h.server_id }),
        )
        .await;
    let id = list["schedules"][0]["id"].as_str().unwrap().to_owned();
    assert_eq!(list["schedules"][0]["serverName"], "SMP");
    assert!(
        list["schedules"][0]["description"]
            .as_str()
            .unwrap()
            .contains("04:00")
    );
    let (status, _) = h
        .call("ScheduleService", "DeleteSchedule", json!({ "id": id }))
        .await;
    assert_eq!(status, StatusCode::OK);
}
