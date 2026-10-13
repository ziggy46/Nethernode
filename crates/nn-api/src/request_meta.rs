//! Per-request client metadata: IP, user agent, and whether the request
//! reached us over HTTPS.

use std::net::SocketAddr;

use axum::extract::{ConnectInfo, Request, State};
use axum::middleware::Next;
use axum::response::Response;
use http::HeaderMap;
use nn_auth::ClientInfo;

use crate::{AppState, SecureCookies};

/// Client metadata attached to every request's extensions.
#[derive(Debug, Clone, Default)]
pub struct RequestMeta {
    /// Client IP, from the socket or (when trusted) proxy headers.
    pub ip: Option<String>,
    /// `User-Agent` header.
    pub user_agent: Option<String>,
    /// Whether cookies set on this response should be `Secure`.
    pub secure_cookies: bool,
}

impl RequestMeta {
    /// Borrowed view for the auth service.
    pub fn client(&self) -> ClientInfo<'_> {
        ClientInfo {
            ip: self.ip.as_deref(),
            user_agent: self.user_agent.as_deref(),
        }
    }
}

/// Middleware that computes [`RequestMeta`] and stores it in the request extensions.
pub(crate) async fn attach(
    State(state): State<AppState>,
    mut req: Request,
    next: Next,
) -> Response {
    let peer = req
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|c| c.0);
    let headers = req.headers();
    let meta = RequestMeta {
        ip: client_ip(headers, peer, state.config.trust_proxy_headers),
        user_agent: headers
            .get(http::header::USER_AGENT)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned),
        secure_cookies: match state.config.secure_cookies {
            SecureCookies::Always => true,
            SecureCookies::Never => false,
            SecureCookies::Auto => forwarded_https(headers),
        },
    };
    req.extensions_mut().insert(meta);
    next.run(req).await
}

fn client_ip(headers: &HeaderMap, peer: Option<SocketAddr>, trust_proxy: bool) -> Option<String> {
    if trust_proxy {
        let header = |name: &str| headers.get(name).and_then(|v| v.to_str().ok());
        let forwarded = header("cf-connecting-ip")
            .or_else(|| header("x-real-ip"))
            // The proxy appends the address it saw, so the last entry is the
            // only one it vouches for.
            .or_else(|| header("x-forwarded-for").and_then(|v| v.rsplit(',').next()))
            .map(str::trim)
            .filter(|ip| ip.parse::<std::net::IpAddr>().is_ok());
        if let Some(ip) = forwarded {
            return Some(ip.to_owned());
        }
    }
    peer.map(|p| p.ip().to_string())
}

/// True when a proxy reports the original request used HTTPS. Trusting this
/// unconditionally is safe: a spoofed value can only make the client's own
/// cookies stricter.
fn forwarded_https(headers: &HeaderMap) -> bool {
    let proto = headers
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(',').next())
        .map(|v| v.trim().eq_ignore_ascii_case("https"));
    if let Some(is_https) = proto {
        return is_https;
    }
    headers
        .get("forwarded")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.to_ascii_lowercase().contains("proto=https"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(pairs: &[(&'static str, &'static str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (k, v) in pairs {
            map.insert(*k, v.parse().unwrap());
        }
        map
    }

    #[test]
    fn proxy_headers_ignored_unless_trusted() {
        let peer = Some("10.0.0.5:4000".parse().unwrap());
        let h = headers(&[("x-forwarded-for", "1.2.3.4")]);
        assert_eq!(client_ip(&h, peer, false).as_deref(), Some("10.0.0.5"));
        assert_eq!(client_ip(&h, peer, true).as_deref(), Some("1.2.3.4"));
    }

    #[test]
    fn trusted_proxy_header_precedence() {
        let h = headers(&[
            ("x-forwarded-for", "9.9.9.9, 1.2.3.4"),
            ("cf-connecting-ip", "5.6.7.8"),
        ]);
        assert_eq!(client_ip(&h, None, true).as_deref(), Some("5.6.7.8"));
        let h = headers(&[("x-forwarded-for", "9.9.9.9, 1.2.3.4")]);
        assert_eq!(client_ip(&h, None, true).as_deref(), Some("1.2.3.4"));
        let h = headers(&[("x-forwarded-for", "not-an-ip")]);
        assert_eq!(client_ip(&h, None, true), None);
    }

    #[test]
    fn detects_forwarded_https() {
        assert!(forwarded_https(&headers(&[("x-forwarded-proto", "https")])));
        assert!(!forwarded_https(&headers(&[("x-forwarded-proto", "http")])));
        assert!(forwarded_https(&headers(&[(
            "forwarded",
            "for=1.2.3.4;proto=https"
        )])));
        assert!(!forwarded_https(&HeaderMap::new()));
    }
}
