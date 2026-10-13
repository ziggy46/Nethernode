//! `GET /ws/servers/{id}/console`: a server's live console over a WebSocket.
//!
//! On connect the server sends the recent backlog, then new lines batched
//! every 50 ms so heavy log output doesn't become one frame per line. The
//! client sends commands, which run over RCON; the output comes back to that
//! client only, since the server log already shows their effect to everyone.
//! Each command is audit-logged. A ping every 30 s keeps proxies (Cloudflare
//! drops idle connections after ~100 s) from closing the socket.
//!
//! Frames are JSON text. From the server:
//! `{"type":"backlog","lines":[…]}`, `{"type":"lines","lines":[…]}`,
//! `{"type":"skipped","count":n}` (a slow client fell behind),
//! `{"type":"reply","command":"…","output":"…"}`, and
//! `{"type":"error","command":"…","message":"…"}`. From the client:
//! `{"type":"command","command":"…"}`.

use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::Extension;
use axum::extract::ws::rejection::WebSocketUpgradeRejection;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, State};
use axum::response::{IntoResponse, Response};
use futures_util::{SinkExt, StreamExt};
use http::{HeaderMap, StatusCode, header};
use nn_auth::{AuthUser, Permission};
use nn_orchestrator::Orchestrator;
use serde_json::{Value, json};
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::mpsc;

use crate::AppState;
use crate::middleware::CurrentUser;
use crate::rpc::audit;
use crate::rpc::servers::orchestrator_error;

const FLUSH_EVERY: Duration = Duration::from_millis(50);
const PING_EVERY: Duration = Duration::from_secs(30);
/// Lines per frame before flushing early.
const MAX_BATCH: usize = 1000;
/// Commands per second a client may send, with this much burst.
const COMMAND_RATE: f64 = 5.0;
const COMMAND_BURST: f64 = 10.0;

pub(crate) async fn console(
    State(state): State<AppState>,
    Path(server_id): Path<String>,
    user: Option<Extension<CurrentUser>>,
    headers: HeaderMap,
    ws: Result<WebSocketUpgrade, WebSocketUpgradeRejection>,
) -> Response {
    let Some(Extension(CurrentUser(authed))) = user else {
        return (StatusCode::UNAUTHORIZED, "sign in required").into_response();
    };
    if !authed.user.role.can(Permission::ManagePlayers) {
        return (
            StatusCode::FORBIDDEN,
            "your role doesn't allow using the console",
        )
            .into_response();
    }
    // Cookies ride along on cross-site WebSocket requests from the same
    // site (another port, say), so require our own origin.
    if !same_origin(&headers, state.config.trust_proxy_headers) {
        return (
            StatusCode::FORBIDDEN,
            "cross-origin console connections aren't allowed",
        )
            .into_response();
    }
    let Some(orch) = state.servers.orchestrator.clone() else {
        return (StatusCode::SERVICE_UNAVAILABLE, "Docker isn't available").into_response();
    };
    match state.auth.store().find_server(&server_id).await {
        Ok(Some(_)) => {}
        Ok(None) => return (StatusCode::NOT_FOUND, "server not found").into_response(),
        Err(e) => {
            tracing::error!(error = %e, "server lookup failed");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    }
    match ws {
        Ok(ws) => ws.on_upgrade(move |socket| run(socket, state, orch, server_id, authed.user)),
        Err(rejection) => rejection.into_response(),
    }
}

async fn run(
    socket: WebSocket,
    state: AppState,
    orch: Arc<Orchestrator>,
    server_id: String,
    user: AuthUser,
) {
    let (mut sink, mut incoming) = socket.split();
    let (backlog, mut logs) = orch.subscribe_logs(&server_id);
    if sink
        .send(text(&json!({ "type": "backlog", "lines": backlog })))
        .await
        .is_err()
    {
        return;
    }

    let (reply_tx, mut replies) = mpsc::channel::<Value>(16);
    let mut flush = tokio::time::interval(FLUSH_EVERY);
    let mut ping = tokio::time::interval_at(tokio::time::Instant::now() + PING_EVERY, PING_EVERY);
    let mut shutdown = state.shutdown.clone();
    let mut pending: Vec<Arc<str>> = Vec::new();
    let mut tokens = COMMAND_BURST;
    let mut refilled = Instant::now();

    loop {
        let outgoing = tokio::select! {
            line = logs.recv() => match line {
                Ok(line) => {
                    pending.push(line);
                    if pending.len() < MAX_BATCH {
                        continue;
                    }
                    Some(json!({ "type": "lines", "lines": std::mem::take(&mut pending) }))
                }
                Err(RecvError::Lagged(count)) => {
                    let lines = std::mem::take(&mut pending);
                    if !lines.is_empty() && sink.send(text(&json!({ "type": "lines", "lines": lines }))).await.is_err() {
                        break;
                    }
                    Some(json!({ "type": "skipped", "count": count }))
                }
                Err(RecvError::Closed) => break,
            },
            _ = flush.tick() => {
                if pending.is_empty() {
                    continue;
                }
                Some(json!({ "type": "lines", "lines": std::mem::take(&mut pending) }))
            }
            _ = ping.tick() => {
                if sink.send(Message::Ping(Default::default())).await.is_err() {
                    break;
                }
                None
            }
            reply = replies.recv() => reply,
            message = incoming.next() => match message {
                Some(Ok(Message::Text(body))) => {
                    let now = Instant::now();
                    tokens = (tokens + now.duration_since(refilled).as_secs_f64() * COMMAND_RATE).min(COMMAND_BURST);
                    refilled = now;
                    match parse_command(&body) {
                        None => Some(json!({ "type": "error", "command": "", "message": "unrecognized message" })),
                        Some(_) if tokens < 1.0 => Some(json!({
                            "type": "error", "command": "", "message": "slow down: too many commands at once",
                        })),
                        Some(command) => {
                            tokens -= 1.0;
                            spawn_command(&state, &orch, &server_id, &user, command, reply_tx.clone());
                            None
                        }
                    }
                }
                Some(Ok(Message::Close(_)) | Err(_)) | None => break,
                Some(Ok(_)) => None,
            },
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    let _ = sink.send(Message::Close(None)).await;
                    break;
                }
                None
            }
        };
        if let Some(frame) = outgoing
            && sink.send(text(&frame)).await.is_err()
        {
            break;
        }
    }
}

fn spawn_command(
    state: &AppState,
    orch: &Arc<Orchestrator>,
    server_id: &str,
    user: &AuthUser,
    command: String,
    replies: mpsc::Sender<Value>,
) {
    let (state, orch, server_id, user) = (
        state.clone(),
        orch.clone(),
        server_id.to_owned(),
        user.clone(),
    );
    tokio::spawn(async move {
        audit(
            &state,
            &user,
            "console.command",
            ("server", &server_id),
            json!({ "command": command }),
        )
        .await;
        let frame = match orch.command(&server_id, &command).await {
            Ok(output) => json!({ "type": "reply", "command": command, "output": output }),
            Err(e) => {
                let message = orchestrator_error(e)
                    .message
                    .unwrap_or_else(|| "the command failed".into());
                json!({ "type": "error", "command": command, "message": message })
            }
        };
        let _ = replies.send(frame).await;
    });
}

fn parse_command(body: &str) -> Option<String> {
    let value: Value = serde_json::from_str(body).ok()?;
    if value.get("type")?.as_str()? != "command" {
        return None;
    }
    Some(value.get("command")?.as_str()?.to_owned())
}

fn text(value: &Value) -> Message {
    Message::Text(value.to_string().into())
}

/// Whether the `Origin` header names the host the request was sent to.
fn same_origin(headers: &HeaderMap, trust_proxy: bool) -> bool {
    let get = |name: header::HeaderName| headers.get(name).and_then(|v| v.to_str().ok());
    let Some(origin) = get(header::ORIGIN) else {
        return false;
    };
    let Some((_, origin_host)) = origin.split_once("://") else {
        return false;
    };
    let origin_host = origin_host.trim_end_matches('/');
    let mut hosts = vec![get(header::HOST)];
    if trust_proxy {
        hosts.push(
            headers
                .get("x-forwarded-host")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.split(',').next())
                .map(str::trim),
        );
    }
    hosts
        .into_iter()
        .flatten()
        .any(|host| host.eq_ignore_ascii_case(origin_host))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(pairs: &[(&'static str, &'static str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.insert(*k, v.parse().unwrap());
        }
        h
    }

    #[test]
    fn origin_must_match_host() {
        let ok = headers(&[
            ("origin", "http://192.0.2.10:8080"),
            ("host", "192.0.2.10:8080"),
        ]);
        assert!(same_origin(&ok, false));
        let other_port = headers(&[
            ("origin", "http://192.0.2.10:9000"),
            ("host", "192.0.2.10:8080"),
        ]);
        assert!(!same_origin(&other_port, false));
        let missing = headers(&[("host", "192.0.2.10:8080")]);
        assert!(!same_origin(&missing, false));
        let proxied = headers(&[
            ("origin", "https://panel.example.com"),
            ("host", "127.0.0.1:8080"),
            ("x-forwarded-host", "panel.example.com"),
        ]);
        assert!(same_origin(&proxied, true));
        assert!(
            !same_origin(&proxied, false),
            "forwarded host only counts when trusted"
        );
    }

    #[test]
    fn commands() {
        assert_eq!(
            parse_command(r#"{"type":"command","command":"list"}"#).as_deref(),
            Some("list")
        );
        assert_eq!(parse_command(r#"{"type":"other"}"#), None);
        assert_eq!(parse_command("list"), None);
    }
}
