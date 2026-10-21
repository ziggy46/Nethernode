//! Modules' web pages on public addresses.
//!
//! A module with a web page can be opened to anyone in one of two ways:
//!
//! - on the **shared address** for modules' pages (a setting, like
//!   `map.example.com`), at a path of its own: `map.example.com/mc1/`;
//! - on an **address of its own**: a prefix put in front of its server's
//!   address (`map` gives `map.mc1.example.com`), or a whole hostname.
//!
//! Requests for those are passed to the module before the panel's own
//! routes, sign-in and headers (a path's prefix is taken off first). DNS,
//! and any reverse proxy or tunnel in front of the panel, must send the
//! names to the panel. Which names and paths go where is kept in memory and
//! worked out again when modules or routes change, and every half minute.

use std::collections::{BTreeMap, HashMap};
use std::sync::{LazyLock, RwLock};
use std::time::Duration;

use axum::body::Body;
use axum::extract::{Request, State};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use http::{HeaderMap, StatusCode, header};
use nn_store::{Route, Server};

use crate::AppState;

/// Largest request body passed on (a map's requests are small).
const MAX_BODY: usize = 16 * 1024 * 1024;
const REFRESH_EVERY: Duration = Duration::from_secs(30);

/// The shared address for modules' pages (`map.example.com`).
pub(crate) const SHARED_HOST_SETTING: &str = "modules.shared_host";

/// A page on the shared address.
#[derive(Debug, Clone)]
struct Page {
    /// Where the module's web page is (`http://127.0.0.1:8100`).
    target: String,
    /// What the list at `/` calls it.
    title: String,
}

/// Where requests go.
#[derive(Debug, Default)]
struct Table {
    /// Modules' own addresses: hostname → target.
    hosts: HashMap<String, String>,
    /// The shared address, and path → page on it.
    shared: Option<(String, BTreeMap<String, Page>)>,
}

static SITES: LazyLock<RwLock<Table>> = LazyLock::new(Default::default);

fn sites() -> std::sync::RwLockReadGuard<'static, Table> {
    SITES.read().unwrap_or_else(|e| e.into_inner())
}

static CLIENT: LazyLock<reqwest::Client> = LazyLock::new(|| {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .connect_timeout(Duration::from_secs(5))
        .build()
        .unwrap_or_default()
});

/// The addresses players use for a server: its exact routes' hostnames,
/// and `<slug>.<domain>` for wildcard routes that pick servers by slug.
pub(crate) fn server_addresses(server: &Server, routes: &[Route]) -> Vec<String> {
    let mut out: Vec<String> = routes
        .iter()
        .filter(|r| r.server_id.as_deref() == Some(server.id.as_str()) && !r.hostname.contains('*'))
        .map(|r| r.hostname.to_ascii_lowercase())
        .collect();
    out.extend(
        routes
            .iter()
            .filter(|r| r.server_id.is_none())
            .filter_map(|r| r.hostname.strip_prefix("*."))
            .map(|domain| format!("{}.{}", server.slug, domain.to_ascii_lowercase())),
    );
    out.dedup();
    out
}

/// A module's addresses: `web_host` in front of each of the server's
/// addresses, or `web_host` itself when it's a whole hostname.
pub(crate) fn addresses(web_host: &str, server_addresses: &[String]) -> Vec<String> {
    let host = web_host.trim().to_ascii_lowercase();
    if host.contains('.') {
        vec![host]
    } else {
        server_addresses
            .iter()
            .map(|a| format!("{host}.{a}"))
            .collect()
    }
}

/// Checks an address someone typed: a prefix (one label) or a hostname.
/// Returns it lowercased.
pub(crate) fn check_host(input: &str) -> Result<String, String> {
    let host = input.trim().trim_end_matches('.').to_ascii_lowercase();
    let label_ok = |l: &str| {
        (1..=63).contains(&l.len())
            && l.bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
            && !l.starts_with('-')
            && !l.ends_with('-')
    };
    if host.is_empty() || host.len() > 253 || !host.split('.').all(label_ok) {
        return Err(
            "Use letters, digits and dashes: a prefix like map, or a whole hostname like map.example.com."
                .into(),
        );
    }
    Ok(host)
}

/// The shared address for modules' pages, if one is set.
pub(crate) async fn shared_host(store: &nn_store::Store) -> Option<String> {
    store
        .get_setting::<String>(SHARED_HOST_SETTING)
        .await
        .ok()
        .flatten()
        .filter(|h| !h.is_empty())
}

/// Checks a path on the shared address someone typed (one segment, like
/// `mc1`). Returns it lowercased.
pub(crate) fn check_path(input: &str) -> Result<String, String> {
    let path = input.trim().trim_matches('/').to_ascii_lowercase();
    let ok = (1..=63).contains(&path.len())
        && path
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !path.starts_with('-');
    if ok {
        Ok(path)
    } else {
        Err("A path is letters, digits and dashes, like mc1.".into())
    }
}

/// Works out again which names and paths go to which module.
pub async fn refresh(state: &AppState) {
    let store = state.auth.store();
    let (Ok(modules), Ok(servers), Ok(routes)) = (
        store.list_modules(None).await,
        store.list_servers().await,
        store.list_routes().await,
    ) else {
        return;
    };
    let shared = shared_host(store).await;
    let servers: HashMap<&str, &Server> = servers.iter().map(|s| (s.id.as_str(), s)).collect();
    let mut table = Table::default();
    let mut pages = BTreeMap::new();
    for module in modules.iter().filter(|m| m.web_public) {
        let (Some(port), Some(server)) = (module.web_port, servers.get(module.server_id.as_str()))
        else {
            continue;
        };
        let target = state
            .nodes
            .for_server(server)
            .module_address(port)
            .unwrap_or_else(|| format!("http://127.0.0.1:{port}"));
        match (&module.web_path, &module.web_host) {
            (Some(path), _) => {
                let title = match nn_orchestrator::modules::template::find(&module.template) {
                    Some(t) => format!("{} · {}", server.name, t.name),
                    None => server.name.clone(),
                };
                pages.entry(path.clone()).or_insert(Page { target, title });
            }
            (None, Some(host)) => {
                for address in addresses(host, &server_addresses(server, &routes)) {
                    table.hosts.entry(address).or_insert_with(|| target.clone());
                }
            }
            (None, None) => {}
        }
    }
    table.shared = shared.map(|host| (host, pages));
    *SITES.write().unwrap_or_else(|e| e.into_inner()) = table;
}

/// Keeps the table current as servers' addresses change.
pub fn spawn_refresher(state: AppState) {
    tokio::spawn(async move {
        loop {
            refresh(&state).await;
            tokio::time::sleep(REFRESH_EVERY).await;
        }
    });
}

/// The hostname a request is for, without a port.
fn host_of(headers: &HeaderMap, trust_proxy: bool) -> Option<String> {
    let forwarded = trust_proxy
        .then(|| headers.get("x-forwarded-host"))
        .flatten();
    let raw = forwarded
        .or_else(|| headers.get(header::HOST))?
        .to_str()
        .ok()?;
    let raw = raw.split(',').next()?.trim();
    let host = match raw.rsplit_once(':') {
        Some((host, port)) if port.bytes().all(|b| b.is_ascii_digit()) => host,
        _ => raw,
    };
    Some(host.trim_end_matches('.').to_ascii_lowercase())
}

/// Where a request for the shared address goes.
#[derive(Debug, PartialEq, Eq)]
enum Shared {
    /// To this page, with this path and query (the page's prefix taken
    /// off).
    Page(String, String),
    /// To the same path with a slash after it (pages use relative links).
    Slash(String),
    /// The list of pages (`/`), or nothing there (anything else).
    List { found: bool },
}

/// Works out where `path_and_query` on the shared address goes.
fn route_shared(pages: &BTreeMap<String, Page>, path_and_query: &str) -> Shared {
    let (path, query) = match path_and_query.split_once('?') {
        Some((p, q)) => (p, Some(q)),
        None => (path_and_query, None),
    };
    let rest = path.trim_start_matches('/');
    let (first, after) = match rest.split_once('/') {
        Some((first, after)) => (first, Some(after)),
        None => (rest, None),
    };
    let first = first.to_ascii_lowercase();
    if !pages.contains_key(&first) {
        return Shared::List {
            found: rest.is_empty(),
        };
    }
    let query = query.map(|q| format!("?{q}")).unwrap_or_default();
    match after {
        None => Shared::Slash(format!("/{first}/{query}")),
        Some(after) => Shared::Page(first, format!("/{after}{query}")),
    }
}

/// The list at the shared address's `/`: the pages that are open.
fn list_page(host: &str, pages: &BTreeMap<String, Page>, found: bool) -> Response {
    let escape = |s: &str| {
        s.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('"', "&quot;")
    };
    let items: String = pages
        .iter()
        .map(|(path, page)| {
            format!(
                "<li><a href=\"/{p}/\">{t}</a></li>",
                p = escape(path),
                t = escape(&page.title)
            )
        })
        .collect();
    let body = format!(
        "<!doctype html><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width\">\
         <title>{h}</title><style>body{{font:16px system-ui,sans-serif;max-width:40rem;margin:3rem auto;padding:0 1rem}}</style>\
         <h1>{h}</h1>{note}<ul>{items}</ul>",
        h = escape(host),
        note = if found {
            ""
        } else {
            "<p>There's nothing at that address.</p>"
        },
        items = if items.is_empty() {
            "<li>Nothing is open here yet.</li>".to_owned()
        } else {
            items
        },
    );
    let status = if found {
        StatusCode::OK
    } else {
        StatusCode::NOT_FOUND
    };
    (
        status,
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        body,
    )
        .into_response()
}

/// Passes requests for modules' addresses to the modules; everything else
/// goes on to the panel.
pub(crate) async fn dispatch(State(state): State<AppState>, req: Request, next: Next) -> Response {
    let Some(host) = host_of(req.headers(), state.config.trust_proxy_headers) else {
        return next.run(req).await;
    };
    enum To {
        Own(String),
        Shared(Shared, Option<String>),
        List(BTreeMap<String, Page>, bool),
    }
    let to = {
        let table = sites();
        if let Some(target) = table.hosts.get(&host) {
            Some(To::Own(target.clone()))
        } else if let Some((shared, pages)) = &table.shared
            && *shared == host
        {
            let path = req.uri().path_and_query().map_or("/", |p| p.as_str());
            Some(match route_shared(pages, path) {
                Shared::List { found } => To::List(pages.clone(), found),
                Shared::Page(page, rest) => {
                    let target = pages.get(&page).map(|p| p.target.clone());
                    To::Shared(Shared::Page(page, rest), target)
                }
                other => To::Shared(other, None),
            })
        } else {
            None
        }
    };
    match to {
        None => next.run(req).await,
        Some(To::Own(target)) => {
            let path = req
                .uri()
                .path_and_query()
                .map_or("/", |p| p.as_str())
                .to_owned();
            forward(&target, &path, req).await
        }
        Some(To::Shared(Shared::Page(_, rest), Some(target))) => forward(&target, &rest, req).await,
        Some(To::Shared(Shared::Slash(to), _)) => {
            (StatusCode::PERMANENT_REDIRECT, [(header::LOCATION, to)]).into_response()
        }
        Some(To::List(pages, found)) => list_page(&host, &pages, found),
        Some(To::Shared(..)) => StatusCode::NOT_FOUND.into_response(),
    }
}

/// Headers that belong to one connection, not passed on.
fn hop_by_hop(name: &header::HeaderName) -> bool {
    matches!(
        name.as_str(),
        "connection"
            | "keep-alive"
            | "proxy-authenticate"
            | "proxy-authorization"
            | "te"
            | "trailer"
            | "transfer-encoding"
            | "upgrade"
            | "host"
    )
}

async fn forward(target: &str, path: &str, req: Request) -> Response {
    let unavailable = || {
        (
            StatusCode::BAD_GATEWAY,
            "This page isn't running right now. Start its module in NetherNode.",
        )
            .into_response()
    };
    let (parts, body) = req.into_parts();
    let Ok(body) = axum::body::to_bytes(body, MAX_BODY).await else {
        return (StatusCode::PAYLOAD_TOO_LARGE, "Too large.").into_response();
    };
    let mut request = CLIENT.request(parts.method, format!("{target}{path}"));
    for (name, value) in &parts.headers {
        if !hop_by_hop(name) {
            request = request.header(name, value);
        }
    }
    let Ok(response) = request.body(body).send().await else {
        return unavailable();
    };
    let mut out = Response::builder().status(response.status());
    for (name, value) in response.headers() {
        if !hop_by_hop(name) {
            out = out.header(name, value);
        }
    }
    let chunks = futures_util::stream::unfold(response, |mut response| async move {
        match response.chunk().await {
            Ok(Some(chunk)) => Some((Ok(chunk), response)),
            Ok(None) => None,
            Err(e) => Some((Err(e), response)),
        }
    });
    out.body(Body::from_stream(chunks))
        .unwrap_or_else(|_| unavailable())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn route(hostname: &str, server_id: Option<&str>) -> Route {
        Route {
            id: hostname.into(),
            server_id: server_id.map(str::to_owned),
            hostname: hostname.into(),
            listener_id: None,
            priority: 0,
            server_name: None,
            server_slug: None,
        }
    }

    #[test]
    fn addresses_follow_the_servers() {
        let server: Server = serde_json::from_value(serde_json::json!({
            "id": "s1", "name": "SMP", "slug": "mc1", "status": "running", "mc_version": "1.21.4",
            "loader": "paper", "loader_version": null, "image": "", "java_version": null,
            "memory_mb": 2048, "cpu_limit": null, "internal_port": 30000, "public_port": null,
            "hostname": null, "auto_start": false, "auto_stop_idle_minutes": null,
            "sleep_when_empty": false, "restart_on_crash": true, "use_aikar_flags": true,
            "motd": null, "max_players": 20, "card_theme": "nether", "container_id": null,
            "rcon_port": null, "rcon_password_enc": null, "eula_accepted_at": null,
            "last_started_at": null, "created_at": 0, "updated_at": 0, "env_json": "{}",
            "mgmt_port": null, "mgmt_secret_enc": null, "modpack_json": null, "node_id": "local"
        }))
        .unwrap();
        let routes = [
            route("*.xtra-pickles.com", None),
            route("play.example.org", Some("s1")),
            route("other.example.org", Some("s2")),
            route("*", Some("s1")),
        ];
        let found = server_addresses(&server, &routes);
        assert_eq!(found, ["play.example.org", "mc1.xtra-pickles.com"]);
        assert_eq!(
            addresses("Map", &found),
            ["map.play.example.org", "map.mc1.xtra-pickles.com"]
        );
        assert_eq!(
            addresses("bluemap.example.net", &found),
            ["bluemap.example.net"]
        );
    }

    #[test]
    fn shared_paths_go_to_their_pages() {
        let page = |t: &str| Page {
            target: t.into(),
            title: t.into(),
        };
        let pages = BTreeMap::from([("mc1".to_owned(), page("a")), ("mc2".to_owned(), page("b"))]);
        assert_eq!(
            route_shared(&pages, "/mc1/maps/world/tiles/0.json?x=1"),
            Shared::Page("mc1".into(), "/maps/world/tiles/0.json?x=1".into())
        );
        assert_eq!(
            route_shared(&pages, "/MC2/"),
            Shared::Page("mc2".into(), "/".into())
        );
        assert_eq!(route_shared(&pages, "/mc1"), Shared::Slash("/mc1/".into()));
        assert_eq!(
            route_shared(&pages, "/mc1?a=b"),
            Shared::Slash("/mc1/?a=b".into())
        );
        assert_eq!(route_shared(&pages, "/"), Shared::List { found: true });
        assert_eq!(
            route_shared(&pages, "/mc3/x"),
            Shared::List { found: false }
        );
        assert!(check_path("/MC1/").is_ok());
        assert!(check_path("mc 1").is_err());
        assert!(check_path("a/b").is_err());
    }

    #[test]
    fn hosts_are_checked() {
        assert_eq!(check_host(" Map ").unwrap(), "map");
        assert_eq!(
            check_host("map.mc1.example.com.").unwrap(),
            "map.mc1.example.com"
        );
        for bad in [
            "",
            "-map",
            "map-",
            "ma p",
            "map..example.com",
            "map_1",
            "map/x",
        ] {
            assert!(check_host(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn the_host_comes_from_the_request() {
        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, "Map.MC1.example.com:8080".parse().unwrap());
        assert_eq!(
            host_of(&headers, false).as_deref(),
            Some("map.mc1.example.com")
        );
        headers.insert("x-forwarded-host", "map.other.com".parse().unwrap());
        assert_eq!(
            host_of(&headers, false).as_deref(),
            Some("map.mc1.example.com")
        );
        assert_eq!(host_of(&headers, true).as_deref(), Some("map.other.com"));
    }
}
