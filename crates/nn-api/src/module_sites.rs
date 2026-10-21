//! Modules' web pages on addresses of their own (`map.mc1.example.com`).
//!
//! A module with a web page can have its own address: a prefix put in front
//! of its server's address (`map`), or a whole hostname. Once it's open to
//! anyone, requests for that hostname are passed to the module as they are,
//! before the panel's own routes, sign-in and headers. DNS (and any reverse
//! proxy in front of the panel) must send the name to the panel.
//!
//! Which hostnames go where is kept in memory and worked out again when
//! modules or routes change, and every half minute.

use std::collections::HashMap;
use std::sync::{LazyLock, RwLock};
use std::time::Duration;

use axum::body::Body;
use axum::extract::{Request, State};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use http::{HeaderMap, StatusCode, header};
use nn_orchestrator::modules::template;
use nn_store::{Module, Route, Server};

use crate::AppState;

/// Largest request body passed on (a map's requests are small).
const MAX_BODY: usize = 16 * 1024 * 1024;
const REFRESH_EVERY: Duration = Duration::from_secs(30);

/// Where a module's address leads.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Target {
    /// Its container's web page (`http://127.0.0.1:8100`).
    Forward(String),
    /// A page NetherNode serves itself: the status page of this module.
    StatusPage(String),
}

/// Hostname → where it leads.
static SITES: LazyLock<RwLock<HashMap<String, Target>>> = LazyLock::new(Default::default);

fn sites() -> std::sync::RwLockReadGuard<'static, HashMap<String, Target>> {
    SITES.read().unwrap_or_else(|e| e.into_inner())
}

/// Whether a module has a web page: its container's, or one NetherNode
/// serves for it.
pub(crate) fn has_page(module: &Module) -> bool {
    module.web_port.is_some() || template::is_panel(&module.template)
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
///
/// Under a domain in `flat_zones` (Cloudflare's, whose free certificate
/// covers only one level below the domain) the prefix joins with a dash, so
/// the name stays one level down: `map` and `mc1.example.com` make
/// `map-mc1.example.com`, not `map.mc1.example.com`.
pub(crate) fn addresses(
    web_host: &str,
    server_addresses: &[String],
    flat_zones: &[String],
) -> Vec<String> {
    let host = web_host.trim().to_ascii_lowercase();
    if host.contains('.') {
        return vec![host];
    }
    server_addresses
        .iter()
        .map(|a| {
            let zone = flat_zones
                .iter()
                .filter(|z| a.ends_with(&format!(".{z}")))
                .max_by_key(|z| z.len());
            match zone {
                Some(zone) => {
                    let below = &a[..a.len() - zone.len() - 1];
                    format!("{host}-{}.{zone}", below.replace('.', "-"))
                }
                None => format!("{host}.{a}"),
            }
        })
        .collect()
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

/// Works out again which hostnames go to which module.
pub async fn refresh(state: &AppState) {
    let store = state.auth.store();
    let (Ok(modules), Ok(servers), Ok(routes)) = (
        store.list_modules(None).await,
        store.list_servers().await,
        store.list_routes().await,
    ) else {
        return;
    };
    let servers: HashMap<&str, &Server> = servers.iter().map(|s| (s.id.as_str(), s)).collect();
    let flat = crate::cloudflare::zones(state).await;
    let mut table = HashMap::new();
    for module in modules.iter().filter(|m| m.web_public) {
        let (Some(host), Some(server)) = (
            module.web_host.as_deref(),
            servers.get(module.server_id.as_str()),
        ) else {
            continue;
        };
        let target = if template::is_panel(&module.template) {
            Target::StatusPage(module.id.clone())
        } else if let Some(port) = module.web_port {
            let node = state.nodes.for_server(server);
            Target::Forward(
                node.module_address(port)
                    .unwrap_or_else(|| format!("http://127.0.0.1:{port}")),
            )
        } else {
            continue;
        };
        for address in addresses(host, &server_addresses(server, &routes), &flat) {
            table.entry(address).or_insert_with(|| target.clone());
        }
    }
    let changed = {
        let mut current = SITES.write().unwrap_or_else(|e| e.into_inner());
        let mut before: Vec<&String> = current.keys().collect();
        let mut after: Vec<&String> = table.keys().collect();
        before.sort();
        after.sort();
        let changed = before != after;
        *current = table;
        changed
    };
    // Cloudflare follows which addresses are open.
    if changed {
        crate::cloudflare::nudge(state);
    }
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

/// Passes requests for a module's address to the module; everything else
/// goes on to the panel.
pub(crate) async fn dispatch(State(state): State<AppState>, req: Request, next: Next) -> Response {
    let target = host_of(req.headers(), state.config.trust_proxy_headers)
        .and_then(|host| sites().get(&host).cloned());
    match target {
        Some(Target::Forward(target)) => forward(&target, req).await,
        Some(Target::StatusPage(module)) => {
            let (method, path) = (req.method().clone(), req.uri().path().to_owned());
            crate::status_page::serve(&state, &module, &method, &path).await
        }
        None => next.run(req).await,
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

async fn forward(target: &str, req: Request) -> Response {
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
    let path = parts.uri.path_and_query().map_or("/", |p| p.as_str());
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
            addresses("Map", &found, &[]),
            ["map.play.example.org", "map.mc1.xtra-pickles.com"]
        );
        assert_eq!(
            addresses("bluemap.example.net", &found, &[]),
            ["bluemap.example.net"]
        );
    }

    #[test]
    fn under_cloudflare_addresses_stay_one_level_down() {
        let zones = ["xtra-pickles.com".to_owned()];
        let found = [
            "mc1.xtra-pickles.com".to_owned(),
            "a.b.xtra-pickles.com".to_owned(),
            "xtra-pickles.com".to_owned(),
            "play.example.org".to_owned(),
        ];
        assert_eq!(
            addresses("map", &found, &zones),
            [
                "map-mc1.xtra-pickles.com",
                "map-a-b.xtra-pickles.com",
                "map.xtra-pickles.com",
                "map.play.example.org"
            ]
        );
        // A whole hostname is used as typed.
        assert_eq!(
            addresses("map.mc1.xtra-pickles.com", &found, &zones),
            ["map.mc1.xtra-pickles.com"]
        );
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
