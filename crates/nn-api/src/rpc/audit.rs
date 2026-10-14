//! AuditService: the audit log, and the dashboard's activity feed.

use std::collections::HashMap;

use connectrpc::{ConnectError, RequestContext, Response, ServiceRequest, ServiceResult};
use nn_auth::Permission;
use nn_store::{AuditEntry, AuditFilter};
use serde_json::Value;

use crate::AppState;
use crate::proto::nethernode::v1 as pb;
use crate::rpc::{require, store_error};

/// Action families the log can be filtered by.
const CATEGORIES: &[&str] = &[
    "auth", "user", "server", "player", "file", "mod", "backup", "schedule", "proxy", "settings",
];

/// What the activity feed shows: things that happened to servers, not
/// sign-ins or account changes.
const ACTIVITY: &[&str] = &[
    "server.create",
    "server.delete",
    "server.start",
    "server.stop",
    "server.restart",
    "server.kill",
    "backup.create",
    "backup.restore",
    "schedule.run",
    "player.ban",
    "player.unban",
    "player.kick",
    "mod.install",
    "mod.update",
];

pub(crate) struct AuditRpc {
    state: AppState,
}

struct Names {
    users: HashMap<String, String>,
    servers: HashMap<String, String>,
}

impl AuditRpc {
    pub(crate) fn new(state: AppState) -> Self {
        Self { state }
    }

    async fn names(&self) -> Result<Names, ConnectError> {
        let store = self.state.auth.store();
        Ok(Names {
            users: store
                .list_users()
                .await
                .map_err(store_error)?
                .into_iter()
                .map(|u| (u.id, u.username))
                .collect(),
            servers: store
                .list_servers()
                .await
                .map_err(store_error)?
                .into_iter()
                .map(|s| (s.id, s.name))
                .collect(),
        })
    }
}

fn details(entry: &AuditEntry) -> Value {
    entry
        .details_json
        .as_deref()
        .and_then(|d| serde_json::from_str(d).ok())
        .unwrap_or(Value::Null)
}

fn text(details: &Value, key: &str) -> Option<String> {
    details.get(key).and_then(Value::as_str).map(str::to_owned)
}

/// The target's name now, or as recorded at the time.
fn target_name(entry: &AuditEntry, details: &Value, names: &Names) -> String {
    let id = entry.target_id.as_deref().unwrap_or_default();
    match entry.target_type.as_deref() {
        Some("server") => names
            .servers
            .get(id)
            .cloned()
            .or_else(|| text(details, "name")),
        Some("user") => names
            .users
            .get(id)
            .cloned()
            .or_else(|| text(details, "username")),
        Some("player") => text(details, "name"),
        _ => None,
    }
    .unwrap_or_default()
}

#[allow(refining_impl_trait)]
impl pb::AuditService for AuditRpc {
    async fn list_audit(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::ListAuditRequest>,
    ) -> ServiceResult<pb::ListAuditResponse> {
        require(&ctx, Permission::ManageSettings)?;
        let prefix;
        let prefixes: Vec<&str> = if request.category.is_empty() {
            Vec::new()
        } else if CATEGORIES.contains(&request.category) {
            prefix = format!("{}.", request.category);
            vec![prefix.as_str()]
        } else {
            return Err(ConnectError::invalid_argument("unknown category"));
        };
        let limit = match request.limit {
            0 => 50,
            n => n.min(200),
        };
        let entries = self
            .state
            .auth
            .store()
            .query_audit(AuditFilter {
                prefixes: &prefixes,
                user_id: request.user_id,
                target_id: request.server_id,
                before_id: request.before_id,
                limit: i64::from(limit),
            })
            .await
            .map_err(store_error)?;
        let names = self.names().await?;
        let next_before_id = (entries.len() == limit as usize)
            .then(|| entries.last().map(|e| e.id.clone()))
            .flatten();
        let mut actors: Vec<pb::AuditActor> = names
            .users
            .iter()
            .map(|(id, username)| pb::AuditActor {
                id: id.clone(),
                username: username.clone(),
                ..Default::default()
            })
            .collect();
        actors.sort_by_key(|a| a.username.to_lowercase());
        let entries = entries
            .into_iter()
            .map(|e| {
                let details = details(&e);
                let user_id = e.user_id.clone().unwrap_or_default();
                pb::AuditEntry {
                    username: names.users.get(&user_id).cloned().unwrap_or_default(),
                    target_name: target_name(&e, &details, &names),
                    details_json: e.details_json.unwrap_or_default(),
                    id: e.id,
                    action: e.action,
                    user_id,
                    target_type: e.target_type.unwrap_or_default(),
                    target_id: e.target_id.unwrap_or_default(),
                    created_at_ms: e.created_at,
                    ..Default::default()
                }
            })
            .collect();
        Response::ok(pb::ListAuditResponse {
            entries,
            next_before_id,
            actors,
            ..Default::default()
        })
    }

    async fn list_activity(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::ListActivityRequest>,
    ) -> ServiceResult<pb::ListActivityResponse> {
        require(&ctx, Permission::View)?;
        let limit = match request.limit {
            0 => 8,
            n => n.min(30),
        };
        let entries = self
            .state
            .auth
            .store()
            .query_audit(AuditFilter {
                prefixes: ACTIVITY,
                limit: i64::from(limit) * 3,
                ..Default::default()
            })
            .await
            .map_err(store_error)?;
        let names = self.names().await?;
        let items = entries
            .into_iter()
            // Prefixes match `server.start` within `server.started`; keep exact actions.
            .filter(|e| ACTIVITY.contains(&e.action.as_str()))
            .take(limit as usize)
            .map(|e| {
                let details = details(&e);
                let server_id = match e.target_type.as_deref() {
                    Some("server") => e.target_id.clone(),
                    _ => text(&details, "server_id"),
                };
                let server_name = server_id
                    .as_ref()
                    .and_then(|id| names.servers.get(id).cloned())
                    .or_else(|| {
                        (e.action == "server.delete")
                            .then(|| text(&details, "name"))
                            .flatten()
                    })
                    .unwrap_or_default();
                let note = match e.action.as_str() {
                    "schedule.run" => [text(&details, "schedule"), text(&details, "result")]
                        .into_iter()
                        .flatten()
                        .collect::<Vec<_>>()
                        .join(": "),
                    a if a.starts_with("player.") => text(&details, "name").unwrap_or_default(),
                    _ => String::new(),
                };
                pb::ActivityItem {
                    actor: e
                        .user_id
                        .as_ref()
                        .and_then(|u| names.users.get(u).cloned())
                        .unwrap_or_default(),
                    server_name,
                    note,
                    id: e.id,
                    action: e.action,
                    created_at_ms: e.created_at,
                    ..Default::default()
                }
            })
            .collect();
        Response::ok(pb::ListActivityResponse {
            items,
            ..Default::default()
        })
    }
}
