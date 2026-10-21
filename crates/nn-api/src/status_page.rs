//! Status pages: a public page for one server, served by NetherNode itself
//! on the status-page module's own address (see [`crate::module_sites`]).
//!
//! The page shows only what the module's settings allow: whether the server
//! is up, how many are playing, its version and MOTD, and (when turned on)
//! the address to join and who's online. It's open to anyone, so nothing
//! here reads the visitor's session. `/status.json` has the same data.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::response::{IntoResponse, Response};
use http::{HeaderValue, Method, StatusCode, header};
use nn_node::Node;
use serde::Serialize;
use serde_json::{Map, Value};

use crate::AppState;
use crate::module_sites;

const PAGE: &str = include_str!("status_page.html");
/// How long a status is reused, so a busy page doesn't query for every
/// visitor.
const FRESH_FOR: Duration = Duration::from_secs(5);
const CSP: &str = "default-src 'none'; style-src 'unsafe-inline'; script-src 'unsafe-inline'; \
                   connect-src 'self'; img-src data:; base-uri 'none'; form-action 'none'; \
                   frame-ancestors 'none'";

/// What the page shows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct Status {
    title: String,
    /// online, starting, stopping, asleep, offline, or unknown (its machine
    /// can't be reached).
    state: &'static str,
    /// From the server's last status ping, while it's online.
    players_online: Option<u32>,
    players_max: Option<u32>,
    /// Who's online, when the page shows names.
    names: Option<Vec<String>>,
    version: String,
    loader: String,
    motd: Option<String>,
    /// Where to join, when the page shows it.
    address: Option<String>,
    /// When it started, while it's online (unix ms).
    since_ms: Option<i64>,
    theme: String,
    checked_ms: i64,
}

/// Module ID → its status and when it was worked out.
type Cache = HashMap<String, (Instant, Arc<Status>)>;

static CACHE: LazyLock<Mutex<Cache>> = LazyLock::new(Default::default);

/// Answers a request for a status page's address.
pub(crate) async fn serve(
    state: &AppState,
    module_id: &str,
    method: &Method,
    path: &str,
) -> Response {
    if !matches!(*method, Method::GET | Method::HEAD) {
        return (StatusCode::METHOD_NOT_ALLOWED, "Only GET.").into_response();
    }
    let json = match path {
        "/" => false,
        "/status.json" => true,
        _ => return (StatusCode::NOT_FOUND, "Not found.").into_response(),
    };
    let Some(status) = cached(state, module_id).await else {
        return (StatusCode::NOT_FOUND, "This page isn't open.").into_response();
    };
    let Ok(data) = serde_json::to_string(&*status) else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    let mut response = if json {
        let mut r = Response::new(Body::from(data));
        r.headers_mut().insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        );
        // Anyone can show it on their own site.
        r.headers_mut().insert(
            header::ACCESS_CONTROL_ALLOW_ORIGIN,
            HeaderValue::from_static("*"),
        );
        r
    } else {
        let mut r = Response::new(Body::from(page(&status, &data)));
        r.headers_mut().insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("text/html; charset=utf-8"),
        );
        r.headers_mut().insert(
            header::CONTENT_SECURITY_POLICY,
            HeaderValue::from_static(CSP),
        );
        r
    };
    let headers = response.headers_mut();
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    response
}

/// The page with its first status filled in.
fn page(status: &Status, data: &str) -> String {
    // Inside <script>, `<` could end it early; JSON allows it escaped.
    let data = data
        .replace('<', "\\u003c")
        .replace('\u{2028}', "\\u2028")
        .replace('\u{2029}', "\\u2029");
    PAGE.replace("{{TITLE}}", &escape_html(&status.title))
        .replace("{{DATA}}", &data)
}

fn escape_html(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

async fn cached(state: &AppState, module_id: &str) -> Option<Arc<Status>> {
    {
        let cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((at, status)) = cache.get(module_id)
            && at.elapsed() < FRESH_FOR
        {
            return Some(status.clone());
        }
    }
    let status = Arc::new(status(state, module_id).await?);
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    cache.retain(|_, (at, _)| at.elapsed() < FRESH_FOR);
    cache.insert(module_id.to_owned(), (Instant::now(), status.clone()));
    Some(status)
}

/// A status page module's server, as its settings allow it to be shown.
async fn status(state: &AppState, module_id: &str) -> Option<Status> {
    let store = state.auth.store();
    let module = store.find_module(module_id).await.ok()??;
    if !module.web_public {
        return None;
    }
    let server = store.find_server(&module.server_id).await.ok()??;
    let settings: Map<String, Value> =
        serde_json::from_str(&module.settings_json).unwrap_or_default();
    let on = |key: &str, default: bool| {
        settings
            .get(key)
            .and_then(Value::as_bool)
            .unwrap_or(default)
    };
    let node = state.nodes.for_server(&server);
    let reachable = !matches!(&node, Node::Remote(r) if !r.is_online());
    let shown = if reachable {
        shown_state(&server.status)
    } else {
        "unknown"
    };
    let online = shown == "online";
    let players = online.then(|| node.players(&server.id)).flatten();
    let names = if on("show_players", false) && online {
        let mut names: Vec<String> = store
            .online_player_sessions(Some(&server.id))
            .await
            .ok()?
            .into_iter()
            .map(|s| s.username)
            .collect();
        names.sort_by_key(|n| n.to_lowercase());
        names.dedup();
        Some(names)
    } else {
        None
    };
    let address = if on("show_address", true) {
        let routes = store.list_routes().await.ok()?;
        module_sites::server_addresses(&server, &routes)
            .into_iter()
            .next()
    } else {
        None
    };
    let title = settings
        .get("title")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map_or_else(|| server.name.clone(), str::to_owned);
    Some(Status {
        title,
        state: shown,
        players_online: players.map(|p| p.online),
        players_max: players.map(|p| p.max),
        names,
        version: server.mc_version.clone(),
        loader: loader_name(&server.loader),
        motd: server.motd.clone().filter(|m| !m.trim().is_empty()),
        address,
        since_ms: if online { server.last_started_at } else { None },
        theme: server.card_theme.clone(),
        checked_ms: nn_store::now_ms(),
    })
}

/// A server's status as the page puts it.
fn shown_state(status: &str) -> &'static str {
    match status {
        "running" => "online",
        "starting" => "starting",
        "stopping" => "stopping",
        "sleeping" => "asleep",
        _ => "offline",
    }
}

fn loader_name(loader: &str) -> String {
    match loader {
        "neoforge" => "NeoForge".into(),
        "vanilla" => "Vanilla".into(),
        other => {
            let mut chars = other.chars();
            chars.next().map_or_else(String::new, |first| {
                first.to_uppercase().chain(chars).collect()
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn example() -> Status {
        Status {
            title: "Pickles <SMP>".into(),
            state: "online",
            players_online: Some(2),
            players_max: Some(20),
            names: Some(vec!["Alex".into()]),
            version: "1.21.4".into(),
            loader: "Fabric".into(),
            motd: Some("§aHi</script><script>alert(1)</script>".into()),
            address: Some("mc1.example.com".into()),
            since_ms: Some(0),
            theme: "nether".into(),
            checked_ms: 0,
        }
    }

    #[test]
    fn the_page_cant_be_broken_out_of() {
        let status = example();
        let data = serde_json::to_string(&status).unwrap();
        let html = page(&status, &data);
        assert!(html.contains("<title>Pickles &lt;SMP&gt;</title>"));
        // The MOTD stays inside the data, never closing the script.
        assert_eq!(html.matches("</script>").count(), 2);
        assert!(html.contains("\\u003c/script>\\u003cscript>alert(1)"));
        // And reads back as it was.
        let start = html.find(r#"id="initial">"#).unwrap() + r#"id="initial">"#.len();
        let end = start + html[start..].find("</script>").unwrap();
        let back: Value = serde_json::from_str(&html[start..end]).unwrap();
        assert_eq!(back["motd"], "§aHi</script><script>alert(1)</script>");
    }

    #[test]
    fn states_are_told_apart() {
        assert_eq!(shown_state("running"), "online");
        assert_eq!(shown_state("sleeping"), "asleep");
        assert_eq!(shown_state("crashed"), "offline");
        assert_eq!(shown_state("installing"), "offline");
        assert_eq!(loader_name("fabric"), "Fabric");
        assert_eq!(loader_name("neoforge"), "NeoForge");
    }
}
