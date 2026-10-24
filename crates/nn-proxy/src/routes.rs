//! Hostname → server resolution.
//!
//! Lookup order: exact hostname, then wildcards (longest suffix first), then
//! the default route `*`. Within a kind, a route restricted to this listener
//! beats one for every listener, then a higher `priority` wins. Routes whose
//! server no longer exists are skipped.

use std::collections::HashMap;
use std::sync::Arc;

use crate::config::{BackendSpec, ListenerSpec, RouteSpec};

/// The pattern that matches any hostname.
pub const DEFAULT_PATTERN: &str = "*";

/// An immutable snapshot of routes, swapped atomically on change.
#[derive(Debug, Default)]
pub struct RouteTable {
    exact: HashMap<String, Vec<Candidate>>,
    /// (suffix including the leading dot, candidate), longest suffix first.
    wildcards: Vec<(String, Candidate)>,
    defaults: Vec<Candidate>,
    backends: HashMap<String, Arc<Backend>>,
    by_slug: HashMap<String, String>,
}

#[derive(Debug, Clone)]
struct Candidate {
    pattern: Arc<str>,
    server_id: Option<String>,
    listener_id: Option<String>,
    priority: i64,
}

/// A routable server, plus whether its connections carry a PROXY header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Backend {
    /// The server.
    pub spec: BackendSpec,
    /// True if any PROXY-protocol listener routes here, meaning the backend
    /// must be configured to expect the header on every connection.
    pub expects_proxy_protocol: bool,
}

/// A successful lookup.
#[derive(Debug, Clone)]
pub struct Resolution {
    /// The pattern that matched (for metrics and logs).
    pub route: Arc<str>,
    /// Where to send the player.
    pub backend: Arc<Backend>,
}

impl RouteTable {
    /// Builds a table. Patterns are expected to be normalized (lowercase,
    /// no trailing dot); `*.x` is a wildcard and `*` the default.
    pub fn build(
        routes: &[RouteSpec],
        backends: &[BackendSpec],
        listeners: &[ListenerSpec],
    ) -> Self {
        let mut table = RouteTable::default();

        // Which servers are reachable through a PROXY-protocol listener.
        let pp_listeners: Vec<&str> = listeners
            .iter()
            .filter(|l| l.proxy_protocol)
            .map(|l| l.id.as_str())
            .collect();
        let reachable_via_pp = |route: &RouteSpec| match &route.listener_id {
            Some(id) => pp_listeners.contains(&id.as_str()),
            None => !pp_listeners.is_empty(),
        };
        let mut pp_servers: Vec<&str> = Vec::new();
        let mut pp_all_slugs = false;
        for route in routes.iter().filter(|r| reachable_via_pp(r)) {
            match &route.server_id {
                Some(id) => pp_servers.push(id),
                None => pp_all_slugs = true,
            }
        }

        for spec in backends {
            let expects = pp_all_slugs || pp_servers.contains(&spec.server_id.as_str());
            table
                .by_slug
                .insert(spec.slug.to_ascii_lowercase(), spec.server_id.clone());
            table.backends.insert(
                spec.server_id.clone(),
                Arc::new(Backend {
                    spec: spec.clone(),
                    expects_proxy_protocol: expects,
                }),
            );
        }

        for route in routes {
            let candidate = Candidate {
                pattern: Arc::from(route.pattern.as_str()),
                server_id: route.server_id.clone(),
                listener_id: route.listener_id.clone(),
                priority: route.priority,
            };
            if route.pattern == DEFAULT_PATTERN {
                table.defaults.push(candidate);
            } else if let Some(suffix) = route.pattern.strip_prefix('*') {
                table.wildcards.push((suffix.to_owned(), candidate));
            } else {
                table
                    .exact
                    .entry(route.pattern.clone())
                    .or_default()
                    .push(candidate);
            }
        }

        // A listener's own route first (a server's port of its own), then
        // priority.
        let order = |a: &Candidate, b: &Candidate| {
            b.listener_id
                .is_some()
                .cmp(&a.listener_id.is_some())
                .then(b.priority.cmp(&a.priority))
        };
        for candidates in table.exact.values_mut() {
            candidates.sort_by(order);
        }
        table.defaults.sort_by(order);
        table
            .wildcards
            .sort_by(|(sa, a), (sb, b)| sb.len().cmp(&sa.len()).then(order(a, b)));
        table
    }

    /// Finds where a player who typed `host` on `listener_id` should go.
    pub fn resolve(&self, listener_id: &str, host: &str) -> Option<Resolution> {
        let applies = |c: &&Candidate| c.listener_id.as_deref().is_none_or(|id| id == listener_id);

        if !host.is_empty() {
            if let Some(found) = self.exact.get(host).and_then(|cs| {
                cs.iter()
                    .filter(applies)
                    .find_map(|c| self.backend_for(c, None))
            }) {
                return Some(found);
            }
            for (suffix, candidate) in &self.wildcards {
                let Some(sub) = host.strip_suffix(suffix.as_str()) else {
                    continue;
                };
                if sub.is_empty() || !applies(&candidate) {
                    continue;
                }
                if let Some(found) = self.backend_for(candidate, Some(sub)) {
                    return Some(found);
                }
            }
        }
        self.defaults
            .iter()
            .filter(applies)
            .find_map(|c| self.backend_for(c, None))
    }

    fn backend_for(&self, candidate: &Candidate, subdomain: Option<&str>) -> Option<Resolution> {
        let server_id = match (&candidate.server_id, subdomain) {
            (Some(id), _) => id,
            // Slug wildcard: the subdomain must be exactly one label.
            (None, Some(sub)) if !sub.contains('.') => self.by_slug.get(sub)?,
            (None, _) => return None,
        };
        let backend = self.backends.get(server_id)?;
        Some(Resolution {
            route: candidate.pattern.clone(),
            backend: backend.clone(),
        })
    }

    /// All backends (used by the status poller).
    pub fn backends(&self) -> impl Iterator<Item = &Arc<Backend>> {
        self.backends.values()
    }

    /// Looks up a backend by server ID.
    pub fn backend(&self, server_id: &str) -> Option<&Arc<Backend>> {
        self.backends.get(server_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ServerState;

    fn backend(id: &str, slug: &str) -> BackendSpec {
        BackendSpec {
            remote: false,
            server_id: id.into(),
            name: slug.into(),
            slug: slug.into(),
            addr: "127.0.0.1:30000".parse().unwrap(),
            state: ServerState::Running,
        }
    }

    fn route(pattern: &str, server: Option<&str>) -> RouteSpec {
        RouteSpec {
            id: pattern.into(),
            pattern: pattern.into(),
            server_id: server.map(Into::into),
            listener_id: None,
            priority: 0,
        }
    }

    fn listener(id: &str, pp: bool) -> ListenerSpec {
        ListenerSpec {
            id: id.into(),
            bind: "0.0.0.0:25565".parse().unwrap(),
            proxy_protocol: pp,
            kind: crate::config::ListenerKind::Java,
        }
    }

    fn resolve(table: &RouteTable, host: &str) -> Option<String> {
        table
            .resolve("main", host)
            .map(|r| r.backend.spec.server_id.clone())
    }

    #[test]
    fn exact_wildcard_default_precedence() {
        let backends = [
            backend("smp", "smp"),
            backend("creative", "creative"),
            backend("lobby", "lobby"),
        ];
        let routes = [
            route("smp.example.com", Some("smp")),
            route("*.example.com", Some("creative")),
            route("*", Some("lobby")),
        ];
        let table = RouteTable::build(&routes, &backends, &[]);
        assert_eq!(resolve(&table, "smp.example.com").as_deref(), Some("smp"));
        assert_eq!(
            resolve(&table, "anything.example.com").as_deref(),
            Some("creative")
        );
        assert_eq!(
            resolve(&table, "example.com").as_deref(),
            Some("lobby"),
            "bare domain isn't a subdomain"
        );
        assert_eq!(resolve(&table, "other.net").as_deref(), Some("lobby"));
        assert_eq!(resolve(&table, "").as_deref(), Some("lobby"));
    }

    #[test]
    fn slug_wildcards_route_by_subdomain() {
        let backends = [backend("id-smp", "smp"), backend("id-sky", "sky")];
        let routes = [route("*.mc.example.com", None)];
        let table = RouteTable::build(&routes, &backends, &[]);
        assert_eq!(
            resolve(&table, "smp.mc.example.com").as_deref(),
            Some("id-smp")
        );
        assert_eq!(
            resolve(&table, "sky.mc.example.com").as_deref(),
            Some("id-sky")
        );
        assert_eq!(resolve(&table, "nope.mc.example.com"), None);
        assert_eq!(
            resolve(&table, "a.smp.mc.example.com"),
            None,
            "one label only"
        );
    }

    #[test]
    fn longest_wildcard_suffix_wins() {
        let backends = [backend("a", "a"), backend("b", "b")];
        let routes = [
            route("*.example.com", Some("a")),
            route("*.eu.example.com", Some("b")),
        ];
        let table = RouteTable::build(&routes, &backends, &[]);
        assert_eq!(resolve(&table, "x.eu.example.com").as_deref(), Some("b"));
        assert_eq!(resolve(&table, "x.us.example.com").as_deref(), Some("a"));
    }

    #[test]
    fn listener_restrictions_and_priority() {
        let backends = [backend("a", "a"), backend("b", "b"), backend("c", "c")];
        let mut only_other = route("*", Some("a"));
        only_other.listener_id = Some("other".into());
        let mut low = route("*", Some("b"));
        low.id = "low".into();
        let table = RouteTable::build(&[only_other, low], &backends, &[]);
        assert_eq!(resolve(&table, "x").as_deref(), Some("b"));
        assert_eq!(
            table.resolve("other", "x").unwrap().backend.spec.server_id,
            "a"
        );

        let mut high = route("*.example.com", Some("c"));
        high.priority = 10;
        let mut also = route("*.example.com", Some("a"));
        also.id = "also".into();
        let table = RouteTable::build(&[also, high], &backends, &[]);
        assert_eq!(resolve(&table, "x.example.com").as_deref(), Some("c"));

        // A listener's own route beats a higher-priority one for every
        // listener, on that listener only.
        let mut everywhere = route("*", Some("b"));
        everywhere.priority = 10;
        let mut own = route("*", Some("c"));
        own.id = "own".into();
        own.listener_id = Some("own-port".into());
        let table = RouteTable::build(&[everywhere, own], &backends, &[]);
        assert_eq!(
            table
                .resolve("own-port", "x")
                .unwrap()
                .backend
                .spec
                .server_id,
            "c"
        );
        assert_eq!(resolve(&table, "x").as_deref(), Some("b"));
    }

    #[test]
    fn skips_routes_to_missing_servers() {
        let routes = [
            route("play.example.com", Some("gone")),
            route("*", Some("lobby")),
        ];
        let table = RouteTable::build(&routes, &[backend("lobby", "lobby")], &[]);
        assert_eq!(
            resolve(&table, "play.example.com").as_deref(),
            Some("lobby")
        );
        let table = RouteTable::build(&routes[..1], &[], &[]);
        assert_eq!(resolve(&table, "play.example.com"), None);
    }

    #[test]
    fn proxy_protocol_expectation_follows_listeners() {
        let backends = [backend("a", "a"), backend("b", "b")];
        let mut via_pp = route("a.example.com", Some("a"));
        via_pp.listener_id = Some("pp".into());
        let mut via_plain = route("b.example.com", Some("b"));
        via_plain.listener_id = Some("plain".into());
        let listeners = [listener("pp", true), listener("plain", false)];
        let table = RouteTable::build(&[via_pp, via_plain], &backends, &listeners);
        assert!(table.backend("a").unwrap().expects_proxy_protocol);
        assert!(!table.backend("b").unwrap().expects_proxy_protocol);
    }
}
