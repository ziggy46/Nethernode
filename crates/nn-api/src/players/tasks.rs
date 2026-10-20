//! Background player work: lifting expired bans, watching running servers'
//! management protocol for list changes, and applying bans that cover every
//! server to each one before it starts.

use std::sync::Arc;
use std::time::Duration;

use nn_events::Event;
use nn_store::{NewAuditEntry, Server};

use super::backend::{Ctx, Via};
use super::resolve;
use crate::AppState;

const EXPIRY_INTERVAL: Duration = Duration::from_secs(30);

/// Starts the background tasks.
pub fn spawn(state: AppState) {
    // On every machine: other machines' servers get it from the panel,
    // through the link, before their agent starts them.
    state.nodes.set_before_start(Arc::new(GlobalBans {
        state: state.clone(),
    }));

    let expiry = state.clone();
    tokio::spawn(async move {
        expiry.players.bans.reload(expiry.auth.store()).await;
        let mut tick = tokio::time::interval(EXPIRY_INTERVAL);
        loop {
            tick.tick().await;
            lift_expired(&expiry).await;
            expiry.players.bans.reload(expiry.auth.store()).await;
        }
    });

    tokio::spawn(async move {
        let mut events = state.events.subscribe();
        loop {
            match events.recv().await {
                Ok(Event::ServerStatusChanged { server_id, status }) => {
                    if status == "running" {
                        tokio::spawn(watch(state.clone(), server_id));
                    } else {
                        state.players.forget_mgmt(&server_id).await;
                    }
                }
                Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    });
}

/// Forwards a running server's management notifications as
/// `players_changed` events until the connection ends.
async fn watch(state: AppState, server_id: String) {
    let Ok(ctx) = Ctx::open(&state, &server_id).await else {
        return;
    };
    let Via::Mgmt(client) = &ctx.via else { return };
    let mut notes = client.notifications();
    while let Ok(note) = notes.recv().await {
        tracing::debug!(server = %server_id, method = %note.method, "management notification");
        state.events.publish(Event::PlayersChanged {
            server_id: Some(server_id.clone()),
        });
    }
}

/// Lifts bans past their expiry. Servers that can't be reached right now
/// (starting, stopping) are retried on the next pass.
async fn lift_expired(state: &AppState) {
    let store = state.auth.store();
    let Ok(expired) = store.expired_player_bans().await else {
        return;
    };
    for ban in expired {
        let servers = match &ban.server_id {
            Some(id) => vec![id.clone()],
            None => match store.list_servers().await {
                Ok(all) => all.into_iter().map(|s| s.id).collect(),
                Err(_) => continue,
            },
        };
        let mut finished = true;
        for server_id in servers {
            let Ok(ctx) = Ctx::open(state, &server_id).await else {
                continue;
            };
            if matches!(ctx.via, Via::Unavailable(_)) {
                finished = false;
                continue;
            }
            let uuid = if ctx.online_mode {
                ban.uuid.as_deref()
            } else {
                None
            };
            let Ok(player) = resolve::profile(&ctx, &ban.name, uuid).await else {
                continue;
            };
            if let Err(e) = ctx.pardon(&player).await {
                tracing::warn!(server = %server_id, player = %ban.name, error = %e.message.as_deref().unwrap_or(""), "couldn't lift an expired ban");
                finished = false;
            }
            state.events.publish(Event::PlayersChanged {
                server_id: Some(server_id),
            });
        }
        if finished && store.lift_player_ban(&ban.id).await.unwrap_or(false) {
            tracing::info!(player = %ban.name, "ban expired and was lifted");
            let entry = NewAuditEntry {
                user_id: None,
                action: "player.ban_expired",
                target_type: Some("player"),
                target_id: ban.uuid.as_deref(),
                details: Some(serde_json::json!({ "name": ban.name, "server_id": ban.server_id })),
            };
            if let Err(e) = store.insert_audit(entry).await {
                tracing::warn!(error = %e, "couldn't record ban expiry");
            }
        }
    }
}

/// Brings a server's ban list up to date before it starts: applies bans that
/// cover every server (so servers created or offline when the ban was placed
/// get it too) and drops NetherNode's bans that were lifted while it couldn't
/// remove them itself.
struct GlobalBans {
    state: AppState,
}

impl nn_orchestrator::BeforeStart for GlobalBans {
    fn before_start<'a>(
        &'a self,
        server: &'a Server,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            let Ok(bans) = self
                .state
                .auth
                .store()
                .active_player_bans(Some(&server.id))
                .await
            else {
                return;
            };
            let ctx = Ctx::open_stopped(&self.state, server.clone()).await;
            let active = bans
                .iter()
                .map(|b| (b.uuid.clone(), b.name.clone()))
                .collect();
            if let Err(e) = ctx.drop_lifted_bans(active).await {
                tracing::warn!(server = %server.id, error = %e.message.as_deref().unwrap_or(""), "couldn't remove lifted bans");
            }
            for ban in bans.into_iter().filter(|b| b.server_id.is_none()) {
                let uuid = if ctx.online_mode {
                    ban.uuid.as_deref()
                } else {
                    None
                };
                let Ok(player) = resolve::profile(&ctx, &ban.name, uuid).await else {
                    continue;
                };
                if let Err(e) = ctx
                    .ban(&player, ban.reason.as_deref(), ban.expires_at)
                    .await
                {
                    tracing::warn!(server = %server.id, player = %ban.name, error = %e.message.as_deref().unwrap_or(""), "couldn't apply a ban for every server");
                }
            }
        })
    }
}
