//! The parts of Cloudflare's API NetherNode uses: accounts, zones, tunnels
//! and DNS records. <https://developers.cloudflare.com/api/>

use std::time::Duration;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// Cloudflare's API, unless the configuration says otherwise (tests).
pub(crate) const DEFAULT_BASE: &str = "https://api.cloudflare.com/client/v4";

/// An account the token can use.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct Account {
    pub id: String,
    pub name: String,
}

/// A zone (domain) the token can see.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct Zone {
    pub id: String,
    pub name: String,
    /// The account it belongs to.
    #[serde(default)]
    pub account: Option<Account>,
}

/// A DNS record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Record {
    #[serde(default, skip_serializing)]
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub name: String,
    pub content: String,
    #[serde(default)]
    pub proxied: bool,
    #[serde(default)]
    pub comment: Option<String>,
}

#[derive(Deserialize)]
struct Envelope<T> {
    #[serde(default)]
    success: bool,
    #[serde(default)]
    errors: Vec<ApiError>,
    result: Option<T>,
    #[serde(default)]
    result_info: Option<ResultInfo>,
}

#[derive(Deserialize)]
struct ApiError {
    #[serde(default)]
    code: i64,
    #[serde(default)]
    message: String,
}

#[derive(Deserialize)]
struct ResultInfo {
    #[serde(default)]
    total_pages: Option<u32>,
}

/// Cloudflare's API with one token.
pub(crate) struct Api {
    http: reqwest::Client,
    base: String,
    token: String,
}

impl Api {
    pub(crate) fn new(base: &str, token: &str) -> Self {
        Self {
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(30))
                .user_agent(concat!("NetherNode/", env!("CARGO_PKG_VERSION")))
                .build()
                .unwrap_or_default(),
            base: base.trim_end_matches('/').to_owned(),
            token: token.to_owned(),
        }
    }

    async fn call<T: DeserializeOwned>(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<(T, Option<u32>), String> {
        let mut request = self
            .http
            .request(method, format!("{}{path}", self.base))
            .bearer_auth(&self.token);
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request
            .send()
            .await
            .map_err(|e| format!("couldn't reach Cloudflare: {}", e.without_url()))?;
        let status = response.status();
        let envelope: Envelope<T> = response
            .json()
            .await
            .map_err(|_| format!("Cloudflare answered {status} with something unexpected"))?;
        if !envelope.success {
            let why: Vec<String> = envelope
                .errors
                .iter()
                .map(|e| format!("{} ({})", e.message, e.code))
                .collect();
            return Err(if why.is_empty() {
                format!("Cloudflare answered {status}")
            } else {
                why.join("; ")
            });
        }
        let pages = envelope.result_info.and_then(|i| i.total_pages);
        envelope
            .result
            .map(|r| (r, pages))
            .ok_or_else(|| "Cloudflare's answer was empty".to_owned())
    }

    async fn all<T: DeserializeOwned>(&self, path: &str) -> Result<Vec<T>, String> {
        let mut out = Vec::new();
        let join = if path.contains('?') { '&' } else { '?' };
        for page in 1..=50 {
            let (items, pages): (Vec<T>, _) = self
                .call(
                    reqwest::Method::GET,
                    &format!("{path}{join}per_page=100&page={page}"),
                    None,
                )
                .await?;
            let done = items.is_empty() || pages.is_none_or(|p| page >= p);
            out.extend(items);
            if done {
                break;
            }
        }
        Ok(out)
    }

    /// Accounts the token can use.
    pub(crate) async fn accounts(&self) -> Result<Vec<Account>, String> {
        self.all("/accounts").await
    }

    /// Zones the token can see.
    pub(crate) async fn zones(&self) -> Result<Vec<Zone>, String> {
        self.all("/zones").await
    }

    /// Makes a tunnel whose routes are set through the API. Returns its ID.
    pub(crate) async fn create_tunnel(&self, account: &str, name: &str) -> Result<String, String> {
        #[derive(Deserialize)]
        struct Tunnel {
            id: String,
        }
        let (tunnel, _): (Tunnel, _) = self
            .call(
                reqwest::Method::POST,
                &format!("/accounts/{account}/cfd_tunnel"),
                Some(json!({ "name": name, "config_src": "cloudflare" })),
            )
            .await?;
        Ok(tunnel.id)
    }

    /// The token its connector runs with.
    pub(crate) async fn tunnel_token(&self, account: &str, tunnel: &str) -> Result<String, String> {
        let (token, _): (String, _) = self
            .call(
                reqwest::Method::GET,
                &format!("/accounts/{account}/cfd_tunnel/{tunnel}/token"),
                None,
            )
            .await?;
        Ok(token)
    }

    /// Sets the tunnel's routes: each hostname to `service`, and nothing
    /// else.
    pub(crate) async fn set_routes(
        &self,
        account: &str,
        tunnel: &str,
        hostnames: &[String],
        service: &str,
    ) -> Result<(), String> {
        let mut ingress: Vec<Value> = hostnames
            .iter()
            .map(|h| json!({ "hostname": h, "service": service }))
            .collect();
        ingress.push(json!({ "service": "http_status:404" }));
        let _: (Value, _) = self
            .call(
                reqwest::Method::PUT,
                &format!("/accounts/{account}/cfd_tunnel/{tunnel}/configurations"),
                Some(json!({ "config": { "ingress": ingress } })),
            )
            .await?;
        Ok(())
    }

    /// Deletes a tunnel (dropping its connections first).
    pub(crate) async fn delete_tunnel(&self, account: &str, tunnel: &str) -> Result<(), String> {
        let _ = self
            .call::<Value>(
                reqwest::Method::DELETE,
                &format!("/accounts/{account}/cfd_tunnel/{tunnel}/connections"),
                None,
            )
            .await;
        let _: (Value, _) = self
            .call(
                reqwest::Method::DELETE,
                &format!("/accounts/{account}/cfd_tunnel/{tunnel}"),
                None,
            )
            .await?;
        Ok(())
    }

    /// A zone's DNS records.
    pub(crate) async fn records(&self, zone: &str) -> Result<Vec<Record>, String> {
        self.all(&format!("/zones/{zone}/dns_records")).await
    }

    /// Adds a DNS record.
    pub(crate) async fn create_record(&self, zone: &str, record: &Record) -> Result<(), String> {
        let _: (Value, _) = self
            .call(
                reqwest::Method::POST,
                &format!("/zones/{zone}/dns_records"),
                Some(record_body(record)),
            )
            .await?;
        Ok(())
    }

    /// Replaces a DNS record.
    pub(crate) async fn update_record(
        &self,
        zone: &str,
        id: &str,
        record: &Record,
    ) -> Result<(), String> {
        let _: (Value, _) = self
            .call(
                reqwest::Method::PUT,
                &format!("/zones/{zone}/dns_records/{id}"),
                Some(record_body(record)),
            )
            .await?;
        Ok(())
    }

    /// Removes a DNS record.
    pub(crate) async fn delete_record(&self, zone: &str, id: &str) -> Result<(), String> {
        let _: (Value, _) = self
            .call(
                reqwest::Method::DELETE,
                &format!("/zones/{zone}/dns_records/{id}"),
                None,
            )
            .await?;
        Ok(())
    }
}

fn record_body(record: &Record) -> Value {
    json!({
        "type": record.kind,
        "name": record.name,
        "content": record.content,
        "proxied": record.proxied,
        // Automatic.
        "ttl": 1,
        "comment": record.comment,
    })
}
