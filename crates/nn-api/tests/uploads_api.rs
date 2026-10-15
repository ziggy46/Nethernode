//! Resumable uploads (tus) end to end: creation checks, chunking, resume,
//! checksums, limits, placement, cancellation, and cleanup.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::body::{Body, Bytes};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use http::{Request, Response, StatusCode, header};
use http_body_util::BodyExt;
use nn_api::uploads::{UploadSettings, Uploads};
use nn_api::{ApiConfig, AppState, ProxyRuntime, StoreProxyHooks};
use nn_auth::{Auth, AuthConfig, password};
use nn_events::{Event, EventBus};
use nn_node::{LocalNode, Nodes};
use nn_store::{NewServer, NewUser, Store};
use sha2::{Digest, Sha256};
use tower::ServiceExt;

const CSRF: &str = "csrf";
const PASSWORD: &str = "correct horse battery";
const KIB: u64 = 1024;

struct Harness {
    app: Router,
    state: AppState,
    store: Store,
    owner: String,
    root: PathBuf,
    temp: PathBuf,
    server_id: String,
    _tmp: tempfile::TempDir,
}

fn settings() -> UploadSettings {
    UploadSettings {
        chunk_limit: 64 * KIB,
        max_file: 512 * KIB,
        quota: 768 * KIB,
        expiry: Duration::from_secs(3600),
    }
}

async fn harness_with(settings: UploadSettings) -> Harness {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path().canonicalize().unwrap();
    let servers_dir = base.join("servers");
    let temp = base.join("tmp/uploads");
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
    let state = AppState {
        proxy: Arc::new(ProxyRuntime::new(proxy, store.clone(), true)),
        auth: auth.clone(),
        events: events.clone(),
        config: Arc::new(ApiConfig::default()),
        metrics: None,
        shutdown: shutdown_rx,
        players: Default::default(),
        uploads: Arc::new(Uploads::new(
            settings,
            nn_uploads::TempStore::open(&temp).unwrap(),
        )),
        content: Default::default(),
        notifier: Default::default(),
        install_count: Default::default(),
        agents: Default::default(),
        nodes: Nodes::new(
            LocalNode::without_docker("not connected", Some(servers_dir.clone()), events.clone()),
            Some(store.clone()),
        ),
    };
    let app = nn_api::router(state.clone());
    let owner = auth
        .setup("owner", PASSWORD, Default::default())
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
    std::fs::create_dir_all(root.join("world")).unwrap();
    std::fs::write(root.join("server.properties"), "motd=hi\n").unwrap();
    Harness {
        app,
        state,
        store,
        owner,
        root,
        temp,
        server_id,
        _tmp: tmp,
    }
}

async fn harness() -> Harness {
    harness_with(settings()).await
}

fn meta(pairs: &[(&str, Option<&str>)]) -> String {
    pairs
        .iter()
        .map(|(k, v)| match v {
            Some(v) => format!("{k} {}", B64.encode(v)),
            None => (*k).to_owned(),
        })
        .collect::<Vec<_>>()
        .join(",")
}

impl Harness {
    fn request(&self, method: &str, uri: &str, session: &str) -> http::request::Builder {
        Request::builder()
            .method(method)
            .uri(uri)
            .header(
                header::COOKIE,
                format!("nn_csrf={CSRF}; nn_session={session}"),
            )
            .header("x-csrf-token", CSRF)
            .header("tus-resumable", "1.0.0")
    }

    async fn send(&self, req: Request<Body>) -> Response<Body> {
        self.app.clone().oneshot(req).await.unwrap()
    }

    /// Starts an upload; returns the response.
    async fn create_as(
        &self,
        session: &str,
        length: u64,
        pairs: &[(&str, Option<&str>)],
    ) -> Response<Body> {
        let mut all = vec![("server", Some(self.server_id.as_str()))];
        all.extend_from_slice(pairs);
        let req = self
            .request("POST", "/api/uploads", session)
            .header("upload-length", length.to_string())
            .header("upload-metadata", meta(&all))
            .body(Body::empty())
            .unwrap();
        self.send(req).await
    }

    async fn create(&self, length: u64, pairs: &[(&str, Option<&str>)]) -> String {
        let response = self.create_as(&self.owner, length, pairs).await;
        let status = response.status();
        assert_eq!(status, StatusCode::CREATED, "{}", text(response).await);
        let location = response.headers()[header::LOCATION]
            .to_str()
            .unwrap()
            .to_owned();
        assert!(location.starts_with("/api/uploads/"), "relative Location");
        location
    }

    async fn patch(&self, url: &str, offset: u64, data: &[u8]) -> Response<Body> {
        let req = self
            .request("PATCH", url, &self.owner)
            .header(header::CONTENT_TYPE, "application/offset+octet-stream")
            .header("upload-offset", offset.to_string())
            .body(Body::from(data.to_vec()))
            .unwrap();
        self.send(req).await
    }

    async fn head(&self, url: &str) -> Response<Body> {
        let req = self
            .request("HEAD", url, &self.owner)
            .body(Body::empty())
            .unwrap();
        self.send(req).await
    }

    /// Uploads `data` in chunks and waits for the result.
    async fn upload(&self, data: &[u8], pairs: &[(&str, Option<&str>)]) -> Event {
        let mut events = self.state.events.subscribe();
        let url = self.create(data.len() as u64, pairs).await;
        let mut offset = 0;
        for chunk in data.chunks(64 * KIB as usize) {
            let response = self.patch(&url, offset, chunk).await;
            assert_eq!(response.status(), StatusCode::NO_CONTENT);
            offset += chunk.len() as u64;
            assert_eq!(offset_of(&response), offset);
        }
        if data.is_empty() {
            assert_eq!(
                self.patch(&url, 0, b"").await.status(),
                StatusCode::NO_CONTENT
            );
        }
        finished(&mut events).await
    }

    async fn manager(&self, name: &str) -> String {
        let hash = password::hash_password(PASSWORD).unwrap();
        self.store
            .create_user(NewUser {
                username: name,
                password_hash: &hash,
                role: "admin",
                must_change_password: false,
            })
            .await
            .unwrap();
        self.state
            .auth
            .login(name, PASSWORD, Default::default())
            .await
            .unwrap()
            .session()
            .unwrap()
            .token
    }
}

fn offset_of(response: &Response<Body>) -> u64 {
    response.headers()["upload-offset"]
        .to_str()
        .unwrap()
        .parse()
        .unwrap()
}

async fn text(response: Response<Body>) -> String {
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    String::from_utf8_lossy(&bytes).into_owned()
}

async fn finished(events: &mut tokio::sync::broadcast::Receiver<Event>) -> Event {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Ok(event @ Event::UploadFinished { .. }) = events.recv().await {
                return event;
            }
        }
    })
    .await
    .expect("upload_finished")
}

fn sha_b64(data: &[u8]) -> String {
    B64.encode(Sha256::digest(data))
}

fn data(len: usize) -> Vec<u8> {
    (0..len).map(|i| (i * 31 % 251) as u8).collect()
}

#[tokio::test]
async fn options_describe_the_server() {
    let h = harness().await;
    let req = Request::options("/api/uploads")
        .body(Body::empty())
        .unwrap();
    let response = h.send(req).await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    let headers = response.headers();
    assert_eq!(headers["tus-version"], "1.0.0");
    assert_eq!(headers["tus-max-size"], (512 * KIB).to_string().as_str());
    let extensions = headers["tus-extension"].to_str().unwrap();
    for ext in ["creation", "termination", "checksum", "expiration"] {
        assert!(extensions.contains(ext), "{extensions}");
    }
}

#[tokio::test]
async fn uploads_in_chunks_and_lands_in_place() {
    let h = harness().await;
    let body = data(150 * KIB as usize);
    let event = h
        .upload(
            &body,
            &[("filename", Some("level.dat")), ("dir", Some("world"))],
        )
        .await;
    let Event::UploadFinished { path, failed, .. } = event else {
        unreachable!()
    };
    assert!(!failed);
    assert_eq!(path, "world/level.dat");
    assert_eq!(std::fs::read(h.root.join("world/level.dat")).unwrap(), body);
    assert!(
        std::fs::read_dir(&h.temp).unwrap().next().is_none(),
        "the partial file is gone"
    );
    let audit = h.store.recent_audit(50).await.unwrap();
    assert!(audit.iter().any(|a| a.action == "file.upload"));

    // Empty files work too.
    let event = h.upload(b"", &[("filename", Some("empty.txt"))]).await;
    assert!(matches!(event, Event::UploadFinished { failed: false, .. }));
    assert_eq!(std::fs::read(h.root.join("empty.txt")).unwrap(), b"");
}

#[tokio::test]
async fn offsets_must_match_and_resume_from_head() {
    let h = harness().await;
    let body = data(100 * KIB as usize);
    let url = h
        .create(body.len() as u64, &[("filename", Some("a.bin"))])
        .await;
    let head = h.head(&url).await;
    assert_eq!(head.status(), StatusCode::OK);
    assert_eq!(offset_of(&head), 0);
    assert_eq!(
        head.headers()["upload-length"],
        (100 * KIB).to_string().as_str()
    );
    assert!(head.headers().contains_key("upload-expires"));
    assert_eq!(head.headers()[header::CACHE_CONTROL], "no-store");

    assert_eq!(
        h.patch(&url, 0, &body[..40_000]).await.status(),
        StatusCode::NO_CONTENT
    );
    let wrong = h.patch(&url, 0, &body[..10]).await;
    assert_eq!(wrong.status(), StatusCode::CONFLICT);
    assert_eq!(offset_of(&wrong), 40_000);

    // A connection that drops mid-chunk keeps what arrived.
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Bytes, std::io::Error>>(4);
    tx.send(Ok(Bytes::copy_from_slice(&body[40_000..50_000])))
        .await
        .unwrap();
    tx.send(Err(std::io::Error::other("network gone")))
        .await
        .unwrap();
    drop(tx);
    let stream = tokio_stream::wrappers::ReceiverStream::new(rx);
    let req = h
        .request("PATCH", &url, &h.owner)
        .header(header::CONTENT_TYPE, "application/offset+octet-stream")
        .header("upload-offset", "40000")
        .body(Body::from_stream(stream))
        .unwrap();
    h.send(req).await;
    let resumed = offset_of(&h.head(&url).await);
    assert_eq!(resumed, 50_000, "resumes after the bytes that arrived");

    let mut events = h.state.events.subscribe();
    assert_eq!(
        h.patch(&url, resumed, &body[resumed as usize..])
            .await
            .status(),
        StatusCode::NO_CONTENT
    );
    assert!(matches!(
        finished(&mut events).await,
        Event::UploadFinished { failed: false, .. }
    ));
    assert_eq!(std::fs::read(h.root.join("a.bin")).unwrap(), body);

    // Retrying the last chunk after it all arrived is harmless.
    let again = h.patch(&url, body.len() as u64, b"").await;
    assert_eq!(again.status(), StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn one_writer_at_a_time() {
    let h = harness().await;
    let url = h.create(1000, &[("filename", Some("a.bin"))]).await;
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Bytes, std::io::Error>>(1);
    tx.send(Ok(Bytes::from_static(b"abc"))).await.unwrap();
    let req = h
        .request("PATCH", &url, &h.owner)
        .header(header::CONTENT_TYPE, "application/offset+octet-stream")
        .header("upload-offset", "0")
        .body(Body::from_stream(
            tokio_stream::wrappers::ReceiverStream::new(rx),
        ))
        .unwrap();
    let app = h.app.clone();
    let first = tokio::spawn(async move { app.oneshot(req).await.unwrap() });
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(h.patch(&url, 0, b"x").await.status(), StatusCode::LOCKED);
    drop(tx);
    assert_eq!(first.await.unwrap().status(), StatusCode::NO_CONTENT);
    assert_eq!(offset_of(&h.head(&url).await), 3);
}

#[tokio::test]
async fn destinations_are_checked_before_any_bytes_move() {
    let h = harness().await;
    let refused = async |pairs: &[(&str, Option<&str>)]| {
        let response = h.create_as(&h.owner, 10, pairs).await;
        let status = response.status();
        (status, text(response).await)
    };
    for (pairs, why) in [
        (
            vec![("filename", Some("x")), ("dir", Some("../.."))],
            "parent traversal",
        ),
        (
            vec![("filename", Some("x")), ("dir", Some("/etc"))],
            "absolute path",
        ),
        (vec![("filename", Some("../x"))], "traversal in the name"),
        (vec![("filename", Some("a/b"))], "a path as the name"),
        (
            vec![("filename", Some("x")), ("dir", Some("server.properties"))],
            "a file as the folder",
        ),
        (
            vec![("filename", Some("x")), ("dir", Some("missing"))],
            "a missing folder",
        ),
        (vec![("dir", Some("world"))], "no name"),
        (
            vec![("filename", Some("x")), ("conflict", Some("maybe"))],
            "bad conflict mode",
        ),
        (
            vec![("filename", Some("x")), ("sha256", Some("abc"))],
            "bad sha256",
        ),
    ] {
        let (status, body) = refused(&pairs).await;
        assert!(
            status == StatusCode::BAD_REQUEST || status == StatusCode::NOT_FOUND,
            "{why}: {status} {body}"
        );
    }
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&h.temp, h.root.join("out")).unwrap();
        let (status, body) = refused(&[("filename", Some("x")), ("dir", Some("out"))]).await;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "a link out of the folder: {body}"
        );
    }
    let other = h
        .send(
            h.request("POST", "/api/uploads", &h.owner)
                .header("upload-length", "10")
                .header(
                    "upload-metadata",
                    meta(&[("server", Some("nope")), ("filename", Some("x"))]),
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(other.status(), StatusCode::NOT_FOUND);
    assert!(
        std::fs::read_dir(&h.temp).unwrap().next().is_none(),
        "nothing was created"
    );
}

#[tokio::test]
async fn protocol_and_access_rules() {
    let h = harness().await;
    // No Tus-Resumable.
    let req = Request::post("/api/uploads")
        .header(
            header::COOKIE,
            format!("nn_csrf={CSRF}; nn_session={}", h.owner),
        )
        .header("x-csrf-token", CSRF)
        .header("upload-length", "10")
        .body(Body::empty())
        .unwrap();
    assert_eq!(h.send(req).await.status(), StatusCode::PRECONDITION_FAILED);
    // No CSRF token.
    let req = Request::post("/api/uploads")
        .header(
            header::COOKIE,
            format!("nn_csrf={CSRF}; nn_session={}", h.owner),
        )
        .header("tus-resumable", "1.0.0")
        .header("upload-length", "10")
        .body(Body::empty())
        .unwrap();
    assert_eq!(h.send(req).await.status(), StatusCode::FORBIDDEN);
    // Unknown length.
    let req = h
        .request("POST", "/api/uploads", &h.owner)
        .header("upload-defer-length", "1")
        .body(Body::empty())
        .unwrap();
    assert_eq!(h.send(req).await.status(), StatusCode::BAD_REQUEST);
    // Signed out.
    let req = Request::post("/api/uploads")
        .header(header::COOKIE, format!("nn_csrf={CSRF}"))
        .header("x-csrf-token", CSRF)
        .header("tus-resumable", "1.0.0")
        .header("upload-length", "10")
        .body(Body::empty())
        .unwrap();
    assert_eq!(h.send(req).await.status(), StatusCode::UNAUTHORIZED);

    // Someone else's upload doesn't exist for you.
    let url = h.create(10, &[("filename", Some("mine.txt"))]).await;
    let other = h.manager("other").await;
    let req = h.request("HEAD", &url, &other).body(Body::empty()).unwrap();
    assert_eq!(h.send(req).await.status(), StatusCode::NOT_FOUND);
    let req = h
        .request("DELETE", &url, &other)
        .body(Body::empty())
        .unwrap();
    assert_eq!(h.send(req).await.status(), StatusCode::NOT_FOUND);

    // Wrong content type.
    let req = h
        .request("PATCH", &url, &h.owner)
        .header(header::CONTENT_TYPE, "application/octet-stream")
        .header("upload-offset", "0")
        .body(Body::from("x"))
        .unwrap();
    assert_eq!(
        h.send(req).await.status(),
        StatusCode::UNSUPPORTED_MEDIA_TYPE
    );
}

#[tokio::test]
async fn checksums_per_chunk_and_per_file() {
    let h = harness().await;
    let url = h.create(8, &[("filename", Some("c.bin"))]).await;
    let checked = |data: &'static [u8], sum: String| {
        h.request("PATCH", &url, &h.owner)
            .header(header::CONTENT_TYPE, "application/offset+octet-stream")
            .header("upload-offset", "0")
            .header("upload-checksum", format!("sha256 {sum}"))
            .body(Body::from(data))
            .unwrap()
    };
    let bad = h.send(checked(b"abcd", sha_b64(b"wxyz"))).await;
    assert_eq!(bad.status().as_u16(), 460);
    assert_eq!(offset_of(&h.head(&url).await), 0, "a bad chunk isn't kept");
    let good = h.send(checked(b"abcd", sha_b64(b"abcd"))).await;
    assert_eq!(good.status(), StatusCode::NO_CONTENT);
    let req = h
        .request("PATCH", &url, &h.owner)
        .header(header::CONTENT_TYPE, "application/offset+octet-stream")
        .header("upload-offset", "4")
        .header("upload-checksum", "md5 AAAA")
        .body(Body::from("efgh"))
        .unwrap();
    assert_eq!(h.send(req).await.status(), StatusCode::BAD_REQUEST);

    // A whole-file SHA-256 that doesn't match: nothing is placed.
    let wrong = hex::encode(Sha256::digest(b"something else"));
    let event = h
        .upload(
            b"payload",
            &[("filename", Some("bad.bin")), ("sha256", Some(&wrong))],
        )
        .await;
    let Event::UploadFinished {
        failed, message, ..
    } = event
    else {
        unreachable!()
    };
    assert!(failed, "{message}");
    assert!(!h.root.join("bad.bin").exists());
    let right = hex::encode(Sha256::digest(b"payload"));
    let event = h
        .upload(
            b"payload",
            &[("filename", Some("good.bin")), ("sha256", Some(&right))],
        )
        .await;
    assert!(matches!(event, Event::UploadFinished { failed: false, .. }));
    assert_eq!(std::fs::read(h.root.join("good.bin")).unwrap(), b"payload");
}

#[tokio::test]
async fn limits() {
    let h = harness().await;
    let big = h
        .create_as(&h.owner, 513 * KIB, &[("filename", Some("big"))])
        .await;
    assert_eq!(big.status(), StatusCode::PAYLOAD_TOO_LARGE);
    let url = h.create(400 * KIB, &[("filename", Some("one"))]).await;
    let over = h
        .create_as(&h.owner, 400 * KIB, &[("filename", Some("two"))])
        .await;
    assert_eq!(
        over.status(),
        StatusCode::PAYLOAD_TOO_LARGE,
        "past the quota"
    );
    let chunk = h.patch(&url, 0, &data(65 * KIB as usize)).await;
    assert_eq!(
        chunk.status(),
        StatusCode::PAYLOAD_TOO_LARGE,
        "chunk over the limit"
    );
    let past_end = {
        let small = h.create(10, &[("filename", Some("small"))]).await;
        h.patch(&small, 0, b"0123456789AB").await
    };
    assert_eq!(past_end.status(), StatusCode::PAYLOAD_TOO_LARGE);
}

#[tokio::test]
async fn conflicts_rename_overwrite_or_skip() {
    let h = harness().await;
    let rename = h
        .upload(b"new", &[("filename", Some("server.properties"))])
        .await;
    let Event::UploadFinished { path, .. } = rename else {
        unreachable!()
    };
    assert_eq!(path, "server (1).properties");
    let skip = h
        .upload(
            b"new",
            &[
                ("filename", Some("server.properties")),
                ("conflict", Some("skip")),
            ],
        )
        .await;
    let Event::UploadFinished {
        failed,
        skipped,
        message,
        ..
    } = skip
    else {
        unreachable!()
    };
    assert!(!failed && skipped, "{message}");
    assert_eq!(
        std::fs::read(h.root.join("server.properties")).unwrap(),
        b"motd=hi\n"
    );
    h.upload(
        b"motd=new\n",
        &[
            ("filename", Some("server.properties")),
            ("conflict", Some("overwrite")),
        ],
    )
    .await;
    assert_eq!(
        std::fs::read(h.root.join("server.properties")).unwrap(),
        b"motd=new\n"
    );
    let folder = h
        .upload(
            b"x",
            &[("filename", Some("world")), ("conflict", Some("overwrite"))],
        )
        .await;
    assert!(
        matches!(folder, Event::UploadFinished { failed: true, .. }),
        "never replaces a folder"
    );
}

#[tokio::test]
async fn cancel_and_list() {
    let h = harness().await;
    let url = h
        .create(100, &[("filename", Some("a.zip")), ("dir", Some("world"))])
        .await;
    h.patch(&url, 0, b"0123456789").await;
    let req = h
        .request("GET", "/api/uploads", &h.owner)
        .body(Body::empty())
        .unwrap();
    let listed: serde_json::Value = serde_json::from_str(&text(h.send(req).await).await).unwrap();
    assert_eq!(listed[0]["filename"], "a.zip");
    assert_eq!(listed[0]["dir"], "world");
    assert_eq!(listed[0]["received"], 10);

    let req = h
        .request("DELETE", &url, &h.owner)
        .body(Body::empty())
        .unwrap();
    assert_eq!(h.send(req).await.status(), StatusCode::NO_CONTENT);
    assert_eq!(h.head(&url).await.status(), StatusCode::GONE);
    assert_eq!(h.patch(&url, 10, b"x").await.status(), StatusCode::GONE);
    assert!(std::fs::read_dir(&h.temp).unwrap().next().is_none());
}

#[tokio::test]
async fn janitor_discards_abandoned_uploads_and_orphans() {
    let h = harness_with(UploadSettings {
        expiry: Duration::from_millis(1),
        ..settings()
    })
    .await;
    let url = h.create(100, &[("filename", Some("slow.bin"))]).await;
    std::fs::write(h.temp.join("ORPHAN"), b"left by a crash").unwrap();
    tokio::time::sleep(Duration::from_millis(20)).await;
    nn_api::uploads::tasks::janitor(&h.state).await;
    assert_eq!(h.head(&url).await.status(), StatusCode::GONE);
    assert!(std::fs::read_dir(&h.temp).unwrap().next().is_none());
}

#[tokio::test]
async fn zips_can_extract_after_upload() {
    let h = harness().await;
    let mut zip = Vec::new();
    {
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(&mut zip));
        writer
            .start_file(
                "datapacks/pack.mcmeta",
                zip::write::SimpleFileOptions::default(),
            )
            .unwrap();
        std::io::Write::write_all(&mut writer, b"{}").unwrap();
        writer.finish().unwrap();
    }
    let mut events = h.state.events.subscribe();
    h.upload(
        &zip,
        &[
            ("filename", Some("pack.zip")),
            ("dir", Some("world")),
            ("extract", None),
        ],
    )
    .await;
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Ok(Event::FileJobFinished {
                failed, message, ..
            }) = events.recv().await
            {
                assert!(!failed, "{message}");
                return;
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(
        std::fs::read(h.root.join("world/datapacks/pack.mcmeta")).unwrap(),
        b"{}"
    );
}

#[tokio::test]
async fn a_connection_dropping_after_the_last_byte_still_finishes() {
    let h = harness().await;
    let url = h.create(10, &[("filename", Some("edge.bin"))]).await;
    let mut events = h.state.events.subscribe();
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Bytes, std::io::Error>>(4);
    tx.send(Ok(Bytes::from_static(b"0123456789")))
        .await
        .unwrap();
    tx.send(Err(std::io::Error::other("dropped at the end")))
        .await
        .unwrap();
    drop(tx);
    let req = h
        .request("PATCH", &url, &h.owner)
        .header(header::CONTENT_TYPE, "application/offset+octet-stream")
        .header("upload-offset", "0")
        .body(Body::from_stream(
            tokio_stream::wrappers::ReceiverStream::new(rx),
        ))
        .unwrap();
    h.send(req).await;
    assert!(matches!(
        finished(&mut events).await,
        Event::UploadFinished { failed: false, .. }
    ));
    assert_eq!(
        std::fs::read(h.root.join("edge.bin")).unwrap(),
        b"0123456789"
    );
    // Asking afterwards reports it complete.
    assert_eq!(offset_of(&h.head(&url).await), 10);
}
