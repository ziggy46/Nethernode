//! Moving a server to another machine. The old machine archives it (the
//! archive stays there as a backup), the panel streams the archive to the
//! new machine, which sets the server up under the same ID and slug, and
//! then the old machine lets go of it. Schedules, routes, backups and
//! players stay attached, since they're the panel's.

use std::pin::Pin;
use std::task::{Context, Poll};

use connectrpc::ConnectError;
use nn_events::Event;
use nn_node::Node;
use nn_store::Server;
use tokio::io::{AsyncRead, ReadBuf};

use crate::AppState;
use crate::backups::{self, Kind};
use crate::rpc::store_error;

/// Checks a move can start, then runs it in the background. Returns the
/// job ID its progress and outcome are reported under.
pub(crate) async fn start_move(
    state: &AppState,
    server: Server,
    to: &str,
    user_id: &str,
) -> Result<String, ConnectError> {
    if to == server.node_id {
        return Err(ConnectError::failed_precondition(
            "The server is already on that machine.",
        ));
    }
    let store = state.auth.store();
    let target = store
        .find_node(to)
        .await
        .map_err(store_error)?
        .ok_or_else(|| ConnectError::not_found("That machine doesn't exist."))?;
    let origin = store
        .find_node(&server.node_id)
        .await
        .map_err(store_error)?
        .map_or_else(|| server.node_id.clone(), |n| n.name);
    let (from, dest) = (state.nodes.for_server(&server), state.nodes.get(to));
    for (node, name) in [(&dest, &target.name), (&from, &origin)] {
        ready(state, node, name)?;
    }
    if !matches!(server.status.as_str(), "stopped" | "crashed" | "sleeping") {
        return Err(ConnectError::failed_precondition(
            "Stop the server before moving it.",
        ));
    }
    // Modules run on the panel's own machine only, for now.
    if !store
        .list_modules(Some(&server.id))
        .await
        .map_err(store_error)?
        .is_empty()
    {
        return Err(ConnectError::failed_precondition(
            "Remove the server's modules before moving it: modules run on the panel's own machine only for now.",
        ));
    }
    let moving = state
        .nodes
        .begin_move(&server.id)
        .ok_or_else(|| ConnectError::failed_precondition("The server is already moving."))?;
    let job_id = nn_store::new_id();
    let (state, job, user, to) = (
        state.clone(),
        job_id.clone(),
        user_id.to_owned(),
        Destination {
            id: to.to_owned(),
            name: target.name,
            node: dest,
        },
    );
    tokio::spawn(async move {
        let result = run(&state, &server, &from, &origin, &to, &user, &job).await;
        drop(moving);
        match &result {
            Ok(message) => tracing::info!(server = %server.id, to = %to.id, "{message}"),
            Err(message) => tracing::warn!(server = %server.id, to = %to.id, "{message}"),
        }
        backups::finished(&state, &server.id, &job, "move", &result);
    });
    Ok(job_id)
}

/// Refuses changes to a server while it moves.
pub(crate) fn not_moving(state: &AppState, server_id: &str) -> Result<(), ConnectError> {
    if state.nodes.is_moving(server_id) {
        return Err(ConnectError::failed_precondition(
            "The server is moving to another machine. Try again once that's done.",
        ));
    }
    Ok(())
}

/// Whether `node` can take part in a move.
fn ready(state: &AppState, node: &Node, name: &str) -> Result<(), ConnectError> {
    match node {
        Node::Local(_) => match node.unavailable() {
            Some(reason) => Err(ConnectError::unavailable(format!(
                "{name} can't run servers right now: {reason}"
            ))),
            None => Ok(()),
        },
        Node::Remote(remote) if remote.is_online() => Ok(()),
        Node::Remote(remote) => Err(ConnectError::failed_precondition(
            if state.agents.live(remote.id()).is_some() {
                format!("{name} runs a different NetherNode version. Update it first.")
            } else {
                format!("{name} is offline.")
            },
        )),
    }
}

struct Destination {
    id: String,
    name: String,
    node: Node,
}

async fn run(
    state: &AppState,
    server: &Server,
    from: &Node,
    origin: &str,
    to: &Destination,
    user_id: &str,
    job_id: &str,
) -> Result<String, String> {
    let progress = |percent: u8| {
        state.events.publish(Event::FileJobProgress {
            server_id: server.id.clone(),
            job_id: job_id.to_owned(),
            kind: "move".into(),
            percent,
        });
    };
    progress(1);

    // 1. Archive it where it is. The archive stays there as a backup.
    let (backup_id, name) = backups::begin(state, server, Kind::Manual, Some(user_id))
        .await
        .map_err(|e| e.message.unwrap_or_default())?;
    let made = from.backup(server, &name, &nn_store::new_id()).await;
    backups::record(state, server, &backup_id, made).await?;
    progress(30);

    // 2. Copy the archive to the new machine.
    let reader = from
        .open_backup(&name)
        .await
        .ok_or("the server's archive disappeared")?;
    let size = reader.size();
    let bytes = reader.from_offset(0).await.map_err(|e| e.to_string())?;
    let counted = Counted {
        inner: bytes,
        done: 0,
        total: size.max(1),
        shown: 30,
        events: state.events.clone(),
        server_id: server.id.clone(),
        job_id: job_id.to_owned(),
    };
    to.node
        .receive_archive(&name, Box::pin(counted), size)
        .await
        .map_err(|e| format!("Couldn't copy {} to {}: {e}", server.name, to.name))?;
    progress(70);

    // 3. Set it up there, then switch the panel over.
    let switched = switch(state, server, to, &name).await;
    // The copy it was set up from isn't needed any more.
    if let Err(e) = to.node.delete_backup(&name).await {
        tracing::warn!(server = %server.id, error = %e, "couldn't remove a moved server's archive");
    }
    switched?;
    progress(95);

    // 4. Let go of it on the old machine.
    Ok(match from.release(server).await {
        Ok(()) => format!(
            "Moved {} to {}. A copy stays on {origin} as a backup.",
            server.name, to.name
        ),
        Err(e) => format!(
            "Moved {} to {}, but its old copy on {origin} couldn't be removed: {e}",
            server.name, to.name
        ),
    })
}

/// Sets the server up on the new machine from archive `name` and points
/// the panel at it there. On failure, nothing is left there and the panel
/// still has it where it was.
async fn switch(
    state: &AppState,
    server: &Server,
    to: &Destination,
    name: &str,
) -> Result<(), String> {
    let store = state.auth.store();
    let avoid = store
        .ports_used_by_others(&server.id)
        .await
        .map_err(|e| e.to_string())?;
    let sent = Server {
        rcon_password_enc: None,
        mgmt_port: None,
        mgmt_secret_enc: None,
        ..server.clone()
    };
    // Its own job, so the restore's progress doesn't show as the move's.
    match to
        .node
        .adopt(&sent, name, &avoid, &nn_store::new_id())
        .await
    {
        Ok(row) => {
            // This machine's orchestrator has already recorded it.
            if !to.node.is_local() {
                store
                    .upsert_server_mirror(&row, &to.id)
                    .await
                    .map_err(|e| e.to_string())?;
            }
            if let Err(e) = state.proxy.reload().await {
                tracing::warn!(error = %e, "couldn't reload the proxy after a move");
            }
            Ok(())
        }
        Err(e) => {
            if to.node.is_local() {
                // Only once this machine took it over (it may have
                // stopped before that).
                let taken = matches!(
                    store.find_server(&server.id).await,
                    Ok(Some(s)) if s.node_id == nn_store::LOCAL_NODE
                );
                if taken {
                    let _ = to.node.release(server).await;
                    let _ = store.upsert_server_mirror(server, &server.node_id).await;
                }
            } else {
                // The agent cleans up after itself; this makes sure.
                let _ = to.node.release(server).await;
            }
            Err(format!(
                "Couldn't set {} up on {}: {e}",
                server.name, to.name
            ))
        }
    }
}

/// Reports progress (30–70%) as an archive is read.
struct Counted {
    inner: Pin<Box<dyn AsyncRead + Send>>,
    done: u64,
    total: u64,
    shown: u8,
    events: nn_events::EventBus,
    server_id: String,
    job_id: String,
}

impl AsyncRead for Counted {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let before = buf.filled().len();
        let polled = self.inner.as_mut().poll_read(cx, buf);
        if let Poll::Ready(Ok(())) = polled {
            self.done += (buf.filled().len() - before) as u64;
            let percent =
                30 + u8::try_from(40 * self.done.min(self.total) / self.total).unwrap_or(40);
            if percent > self.shown {
                self.shown = percent;
                self.events.publish(Event::FileJobProgress {
                    server_id: self.server_id.clone(),
                    job_id: self.job_id.clone(),
                    kind: "move".into(),
                    percent,
                });
            }
        }
        polled
    }
}
