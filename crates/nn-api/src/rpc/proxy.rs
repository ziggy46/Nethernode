//! `nethernode.v1.ProxyService`.

use std::collections::HashMap;
use std::net::IpAddr;
use std::time::{Duration, UNIX_EPOCH};

use connectrpc::{ConnectError, RequestContext, Response, ServiceRequest, ServiceResult};
use nn_auth::Permission;
use nn_mcproto::normalize_hostname;
use nn_proxy::{ListenerState, ListenerStatus};
use nn_store::{ListenerInput, ProxyListener, Route, RouteInput};
use serde_json::json;

use crate::AppState;
use crate::proto::nethernode::v1 as pb;
use crate::rpc::{audit, require, store_error};

pub(crate) struct ProxyRpc {
    state: AppState,
}

impl ProxyRpc {
    pub(crate) fn new(state: AppState) -> Self {
        Self { state }
    }

    async fn reload(&self) -> Result<Vec<ListenerStatus>, ConnectError> {
        self.state.proxy.reload().await.map_err(store_error)
    }

    async fn listener(
        &self,
        id: &str,
        live: &[ListenerStatus],
    ) -> Result<pb::Listener, ConnectError> {
        let listeners = self
            .state
            .auth
            .store()
            .list_listeners()
            .await
            .map_err(store_error)?;
        let found = listeners
            .into_iter()
            .find(|l| l.id == id)
            .ok_or_else(|| ConnectError::not_found("listener not found"))?;
        Ok(proto_listener(found, live, self.state.proxy.enabled()))
    }

    async fn route(&self, id: &str) -> Result<pb::Route, ConnectError> {
        let routes = self
            .state
            .auth
            .store()
            .list_routes()
            .await
            .map_err(store_error)?;
        routes
            .into_iter()
            .find(|r| r.id == id)
            .map(proto_route)
            .ok_or_else(|| ConnectError::not_found("route not found"))
    }
}

/// Route hostnames are stored in the same normalized form the proxy compares
/// against: lowercase, no trailing dot.
pub(crate) fn normalize_pattern(input: &str) -> Result<(String, pb::RouteKind), ConnectError> {
    let raw = input.trim();
    if raw == "*" {
        return Ok(("*".into(), pb::RouteKind::ROUTE_KIND_DEFAULT));
    }
    let (wildcard, host) = match raw.strip_prefix("*.") {
        Some(rest) => (true, rest),
        None => (false, raw),
    };
    let host = normalize_hostname(host);
    let is_ip = host.parse::<IpAddr>().is_ok();
    // IP literals can be exact routes (players connecting by IP) but not wildcards.
    let valid = !host.is_empty() && if is_ip { !wildcard } else { is_dns_name(&host) };
    if !valid {
        return Err(ConnectError::invalid_argument(
            "Enter a hostname like play.example.com, a wildcard like *.example.com, or * for everything else.",
        ));
    }
    if wildcard {
        Ok((format!("*.{host}"), pb::RouteKind::ROUTE_KIND_WILDCARD))
    } else {
        Ok((host, pb::RouteKind::ROUTE_KIND_EXACT))
    }
}

fn is_dns_name(host: &str) -> bool {
    host.len() <= 253
        && host.split('.').all(|label| {
            (1..=63).contains(&label.len())
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        })
}

fn route_kind(pattern: &str) -> pb::RouteKind {
    if pattern == "*" {
        pb::RouteKind::ROUTE_KIND_DEFAULT
    } else if pattern.starts_with("*.") {
        pb::RouteKind::ROUTE_KIND_WILDCARD
    } else {
        pb::RouteKind::ROUTE_KIND_EXACT
    }
}

fn proto_route(r: Route) -> pb::Route {
    pb::Route {
        kind: route_kind(&r.hostname).into(),
        id: r.id,
        hostname: r.hostname,
        server_id: r.server_id,
        listener_id: r.listener_id,
        priority: r.priority,
        server_name: r.server_name.unwrap_or_default(),
        server_slug: r.server_slug.unwrap_or_default(),
        ..Default::default()
    }
}

fn proto_listener(l: ProxyListener, live: &[ListenerStatus], proxy_enabled: bool) -> pb::Listener {
    let (state, error) = if !proxy_enabled {
        (
            pb::ListenerState::LISTENER_STATE_PROXY_DISABLED,
            String::new(),
        )
    } else if !l.enabled {
        (pb::ListenerState::LISTENER_STATE_DISABLED, String::new())
    } else {
        match live.iter().find(|s| s.spec.id == l.id).map(|s| &s.state) {
            Some(ListenerState::Listening(_)) => {
                (pb::ListenerState::LISTENER_STATE_LISTENING, String::new())
            }
            Some(ListenerState::Failed(e)) => (pb::ListenerState::LISTENER_STATE_FAILED, e.clone()),
            None => (
                pb::ListenerState::LISTENER_STATE_FAILED,
                "Invalid bind address.".to_owned(),
            ),
        }
    };
    pb::Listener {
        id: l.id,
        bind_addr: l.bind_addr,
        port: u32::try_from(l.port).unwrap_or_default(),
        enabled: l.enabled,
        proxy_protocol: l.proxy_protocol,
        state: state.into(),
        error,
        ..Default::default()
    }
}

fn conflict(err: nn_store::Error, message: &str) -> ConnectError {
    if err.is_unique_violation() {
        ConnectError::already_exists(message)
    } else {
        store_error(err)
    }
}

#[allow(refining_impl_trait)]
impl pb::ProxyService for ProxyRpc {
    async fn list_listeners(
        &self,
        ctx: RequestContext,
        _request: ServiceRequest<'_, pb::ListListenersRequest>,
    ) -> ServiceResult<pb::ListListenersResponse> {
        require(&ctx, Permission::View)?;
        let live = self.state.proxy.proxy().listener_statuses().await;
        let enabled = self.state.proxy.enabled();
        let listeners = self
            .state
            .auth
            .store()
            .list_listeners()
            .await
            .map_err(store_error)?;
        let listeners = listeners
            .into_iter()
            .map(|l| proto_listener(l, &live, enabled))
            .collect();
        Response::ok(pb::ListListenersResponse {
            listeners,
            ..Default::default()
        })
    }

    async fn upsert_listener(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::UpsertListenerRequest>,
    ) -> ServiceResult<pb::UpsertListenerResponse> {
        let user = require(&ctx, Permission::ManageProxy)?.clone();
        let bind_addr: IpAddr = request.bind_addr.trim().parse().map_err(|_| {
            ConnectError::invalid_argument("Enter an IP address to listen on, like 0.0.0.0 or ::.")
        })?;
        let port = i64::from(request.port);
        if !(1..=65_535).contains(&port) {
            return Err(ConnectError::invalid_argument(
                "Port must be between 1 and 65535.",
            ));
        }
        let input = ListenerInput {
            bind_addr: &bind_addr.to_string(),
            port,
            enabled: request.enabled,
            proxy_protocol: request.proxy_protocol,
        };
        let id = self
            .state
            .auth
            .store()
            .upsert_listener(request.id, input)
            .await
            .map_err(|e| conflict(e, "Another listener already uses that address and port."))?
            .ok_or_else(|| ConnectError::not_found("listener not found"))?;

        let live = self.reload().await?;
        let listener = self.listener(&id, &live).await?;
        let details = json!({ "bind": format!("{bind_addr}:{port}"), "enabled": listener.enabled, "proxy_protocol": listener.proxy_protocol });
        audit(
            &self.state,
            &user,
            "proxy.listener.upsert",
            ("proxy_listener", &id),
            details,
        )
        .await;
        Response::ok(pb::UpsertListenerResponse {
            listener: listener.into(),
            ..Default::default()
        })
    }

    async fn delete_listener(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::DeleteListenerRequest>,
    ) -> ServiceResult<pb::DeleteListenerResponse> {
        let user = require(&ctx, Permission::ManageProxy)?.clone();
        let id = request.id.to_owned();
        if !self
            .state
            .auth
            .store()
            .delete_listener(&id)
            .await
            .map_err(store_error)?
        {
            return Err(ConnectError::not_found("listener not found"));
        }
        self.reload().await?;
        audit(
            &self.state,
            &user,
            "proxy.listener.delete",
            ("proxy_listener", &id),
            json!({}),
        )
        .await;
        Response::ok(pb::DeleteListenerResponse::default())
    }

    async fn list_routes(
        &self,
        ctx: RequestContext,
        _request: ServiceRequest<'_, pb::ListRoutesRequest>,
    ) -> ServiceResult<pb::ListRoutesResponse> {
        require(&ctx, Permission::View)?;
        let routes = self
            .state
            .auth
            .store()
            .list_routes()
            .await
            .map_err(store_error)?;
        Response::ok(pb::ListRoutesResponse {
            routes: routes.into_iter().map(proto_route).collect(),
            ..Default::default()
        })
    }

    async fn upsert_route(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::UpsertRouteRequest>,
    ) -> ServiceResult<pb::UpsertRouteResponse> {
        let user = require(&ctx, Permission::ManageProxy)?.clone();
        let (hostname, kind) = normalize_pattern(request.hostname)?;
        let store = self.state.auth.store();

        let server_id = request.server_id.filter(|s| !s.is_empty());
        match server_id {
            Some(id) => {
                if store.find_server(id).await.map_err(store_error)?.is_none() {
                    return Err(ConnectError::invalid_argument("That server doesn't exist."));
                }
            }
            None if kind != pb::RouteKind::ROUTE_KIND_WILDCARD => {
                return Err(ConnectError::invalid_argument(
                    "Choose a destination server.",
                ));
            }
            None => {}
        }
        let listener_id = request.listener_id.filter(|s| !s.is_empty());
        if let Some(id) = listener_id {
            let listeners = store.list_listeners().await.map_err(store_error)?;
            if !listeners.iter().any(|l| l.id == id) {
                return Err(ConnectError::invalid_argument(
                    "That listener doesn't exist.",
                ));
            }
        }

        let input = RouteInput {
            server_id,
            hostname: &hostname,
            listener_id,
            priority: request.priority,
        };
        let id = store
            .upsert_route(request.id, input)
            .await
            .map_err(|e| conflict(e, "A route for that hostname already exists."))?
            .ok_or_else(|| ConnectError::not_found("route not found"))?;

        self.reload().await?;
        let route = self.route(&id).await?;
        let details =
            json!({ "hostname": hostname, "server_id": server_id, "listener_id": listener_id });
        audit(
            &self.state,
            &user,
            "proxy.route.upsert",
            ("proxy_route", &id),
            details,
        )
        .await;
        Response::ok(pb::UpsertRouteResponse {
            route: route.into(),
            ..Default::default()
        })
    }

    async fn delete_route(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::DeleteRouteRequest>,
    ) -> ServiceResult<pb::DeleteRouteResponse> {
        let user = require(&ctx, Permission::ManageProxy)?.clone();
        let id = request.id.to_owned();
        if !self
            .state
            .auth
            .store()
            .delete_route(&id)
            .await
            .map_err(store_error)?
        {
            return Err(ConnectError::not_found("route not found"));
        }
        self.reload().await?;
        audit(
            &self.state,
            &user,
            "proxy.route.delete",
            ("proxy_route", &id),
            json!({}),
        )
        .await;
        Response::ok(pb::DeleteRouteResponse::default())
    }

    async fn check_dns(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::CheckDnsRequest>,
    ) -> ServiceResult<pb::CheckDnsResponse> {
        require(&ctx, Permission::View)?;
        let (pattern, kind) = normalize_pattern(request.hostname)?;
        let name = match kind {
            pb::RouteKind::ROUTE_KIND_DEFAULT => {
                return Err(ConnectError::invalid_argument(
                    "The default route (*) has no hostname to check.",
                ));
            }
            // Any subdomain works for a wildcard record; pick an unlikely one.
            pb::RouteKind::ROUTE_KIND_WILDCARD => format!("nn-dns-check{}", &pattern[1..]),
            _ => pattern,
        };
        let lookup = tokio::time::timeout(
            Duration::from_secs(5),
            tokio::net::lookup_host(format!("{name}:25565")),
        )
        .await;
        let (addresses, error) = match lookup {
            Ok(Ok(addrs)) => {
                let mut ips: Vec<String> = addrs.map(|a| a.ip().to_string()).collect();
                ips.sort();
                ips.dedup();
                (ips, String::new())
            }
            Ok(Err(e)) => (Vec::new(), format!("No DNS records found ({e}).")),
            Err(_) => (Vec::new(), "DNS lookup timed out.".to_owned()),
        };
        Response::ok(pb::CheckDnsResponse {
            resolved_name: name,
            addresses,
            error,
            ..Default::default()
        })
    }

    async fn list_connections(
        &self,
        ctx: RequestContext,
        _request: ServiceRequest<'_, pb::ListConnectionsRequest>,
    ) -> ServiceResult<pb::ListConnectionsResponse> {
        require(&ctx, Permission::View)?;
        let proxy = self.state.proxy.proxy();
        let routes = proxy.routes();
        let names: HashMap<&str, &str> = routes
            .backends()
            .map(|b| (b.spec.server_id.as_str(), b.spec.name.as_str()))
            .collect();
        let connections = proxy
            .active_connections()
            .into_iter()
            .map(|c| pb::Connection {
                client_ip: c.client.ip().to_string(),
                server_name: names
                    .get(c.server_id.as_str())
                    .map(|n| n.to_string())
                    .unwrap_or_default(),
                hostname: c.hostname,
                server_id: c.server_id,
                username: c.username,
                started_at_ms: c
                    .started_at
                    .duration_since(UNIX_EPOCH)
                    .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
                    .unwrap_or_default(),
                listener_id: c.listener_id,
                route: c.route,
                ..Default::default()
            })
            .collect();
        Response::ok(pb::ListConnectionsResponse {
            connections,
            ..Default::default()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_route_patterns() {
        let ok = |input: &str| normalize_pattern(input).unwrap();
        assert_eq!(
            ok("Play.Example.com."),
            ("play.example.com".into(), pb::RouteKind::ROUTE_KIND_EXACT)
        );
        assert_eq!(
            ok(" *.MC.example.com "),
            (
                "*.mc.example.com".into(),
                pb::RouteKind::ROUTE_KIND_WILDCARD
            )
        );
        assert_eq!(ok("*"), ("*".into(), pb::RouteKind::ROUTE_KIND_DEFAULT));
        assert_eq!(
            ok("203.0.113.5"),
            ("203.0.113.5".into(), pb::RouteKind::ROUTE_KIND_EXACT)
        );
        for bad in [
            "",
            "bad host",
            "-x.com",
            "a..b",
            "*.",
            "**.x.com",
            "*.203.0.113.5",
            "x.com:25565:1",
        ] {
            assert!(normalize_pattern(bad).is_err(), "{bad:?}");
        }
    }
}
