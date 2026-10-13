//! Docker event handling and startup reconciliation.
//!
//! - On startup: adopt running NetherNode containers (without restarting
//!   them), correct stale statuses, log orphans, then auto-start servers.
//! - Then follow Docker events for NetherNode containers: `start` begins
//!   monitoring, `die` records the exit (stopped, sleeping, or crashed, with
//!   the crash-restart policy). After a dropped event stream, reconcile
//!   again so nothing is missed.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bollard::query_parameters::{EventsOptionsBuilder, ListContainersOptionsBuilder};
use futures_util::StreamExt;

use crate::lifecycle::Orchestrator;
use crate::monitor;
use crate::spec::{LABEL_MANAGED, LABEL_SERVER_ID};

/// Starts reconciliation, auto-start, and the event loop.
pub fn spawn_background(orch: &Arc<Orchestrator>) -> tokio::task::JoinHandle<()> {
    let orch = orch.clone();
    tokio::spawn(async move {
        if let Err(error) = reconcile(&orch).await {
            tracing::error!(%error, "initial reconciliation failed");
        }
        auto_start(&orch).await;
        let mut backoff = Duration::from_secs(1);
        loop {
            follow_events(&orch).await;
            tracing::warn!("Docker event stream ended; reconnecting");
            tokio::time::sleep(backoff).await;
            backoff = (backoff * 2).min(Duration::from_secs(30));
            if reconcile(&orch).await.is_ok() {
                backoff = Duration::from_secs(1);
            }
        }
    })
}

struct Found {
    id: String,
    running: bool,
    exit_code: i64,
}

/// Brings stored statuses in line with what Docker actually runs.
pub async fn reconcile(orch: &Arc<Orchestrator>) -> crate::Result<()> {
    let filters = HashMap::from([("label", vec![format!("{LABEL_MANAGED}=true")])]);
    let options = ListContainersOptionsBuilder::default()
        .all(true)
        .filters(&filters)
        .build();
    let containers = orch.docker.list_containers(Some(options)).await?;

    let mut by_server: HashMap<String, Found> = HashMap::new();
    for c in containers {
        let (Some(id), Some(server_id)) =
            (c.id, c.labels.and_then(|l| l.get(LABEL_SERVER_ID).cloned()))
        else {
            continue;
        };
        let running = c.state.map(|s| s.to_string() == "running").unwrap_or(false);
        let exit_code = c.status.as_deref().and_then(parse_exit_code).unwrap_or(0);
        by_server.insert(
            server_id,
            Found {
                id,
                running,
                exit_code,
            },
        );
    }

    for server in orch.store.list_servers().await? {
        let rt = orch.runtime(&server.id);
        match by_server.remove(&server.id) {
            Some(found) if found.running => {
                if !rt.state().monitored() {
                    tracing::info!(server = %server.id, "adopting running container");
                    if server.container_id.as_deref() != Some(found.id.as_str()) {
                        let _ = orch
                            .store
                            .set_container_id(&server.id, Some(&found.id))
                            .await;
                    }
                    if server.status != "running" {
                        orch.set_status(&server.id, "starting").await;
                    }
                    monitor::spawn(orch, &server.id, &found.id, &rt, true);
                }
            }
            Some(found) => {
                if matches!(
                    server.status.as_str(),
                    "running" | "starting" | "stopping" | "installing"
                ) {
                    let status = if found.exit_code != 0 && server.status != "stopping" {
                        "crashed"
                    } else {
                        "stopped"
                    };
                    orch.set_status(&server.id, status).await;
                }
            }
            None => {
                if server.container_id.is_some() {
                    let _ = orch.store.set_container_id(&server.id, None).await;
                }
                if matches!(
                    server.status.as_str(),
                    "running" | "starting" | "stopping" | "installing"
                ) {
                    orch.set_status(&server.id, "stopped").await;
                }
            }
        }
    }
    for (server_id, found) in by_server {
        tracing::warn!(
            %server_id, container = %found.id,
            "found a NetherNode container with no matching server; leaving it untouched"
        );
    }
    Ok(())
}

async fn auto_start(orch: &Arc<Orchestrator>) {
    let Ok(servers) = orch.store.list_servers().await else {
        return;
    };
    for server in servers
        .into_iter()
        .filter(|s| s.auto_start && matches!(s.status.as_str(), "stopped" | "crashed"))
    {
        tracing::info!(server = %server.id, "auto-starting");
        if let Err(e) = orch.start(&server.id).await {
            orch.report_failure(&server.id, "start", &e);
        }
    }
}

async fn follow_events(orch: &Arc<Orchestrator>) {
    let filters = HashMap::from([
        ("type", vec!["container".to_owned()]),
        ("label", vec![format!("{LABEL_MANAGED}=true")]),
        (
            "event",
            vec!["start".to_owned(), "die".to_owned(), "oom".to_owned()],
        ),
    ]);
    let options = EventsOptionsBuilder::default().filters(&filters).build();
    let mut stream = orch.docker.events(Some(options));
    let mut oom: HashMap<String, Instant> = HashMap::new();
    while let Some(item) = stream.next().await {
        let Ok(event) = item else { break };
        let Some(actor) = event.actor else { continue };
        let attrs = actor.attributes.unwrap_or_default();
        let (Some(server_id), Some(container)) = (attrs.get(LABEL_SERVER_ID).cloned(), actor.id)
        else {
            continue;
        };
        match event.action.as_deref() {
            Some("start") => on_start(orch, &server_id, &container),
            Some("oom") => {
                oom.insert(server_id, Instant::now());
            }
            Some("die") => {
                let code = attrs
                    .get("exitCode")
                    .and_then(|c| c.parse().ok())
                    .unwrap_or(0);
                let out_of_memory = oom
                    .remove(&server_id)
                    .is_some_and(|t| t.elapsed() < Duration::from_secs(30));
                on_die(orch, &server_id, code, out_of_memory).await;
            }
            _ => {}
        }
    }
}

/// A container started (by us, or by someone using the Docker CLI).
fn on_start(orch: &Arc<Orchestrator>, server_id: &str, container: &str) {
    let orch = orch.clone();
    let server_id = server_id.to_owned();
    let container = container.to_owned();
    tokio::spawn(async move {
        // Another NetherNode (a test instance, say) may share this Docker
        // host; its containers carry IDs this database doesn't know.
        if orch.load(&server_id).await.is_err() {
            tracing::debug!(server = %server_id, "ignoring a container start for an unknown server");
            return;
        }
        let rt = orch.runtime(&server_id);
        {
            let state = rt.state();
            if state.monitored() || state.launching {
                return;
            }
        }
        tracing::info!(server = %server_id, "container started outside NetherNode; monitoring it");
        orch.set_status(&server_id, "starting").await;
        monitor::spawn(&orch, &server_id, &container, &rt, true);
    });
}

/// A container exited.
async fn on_die(orch: &Arc<Orchestrator>, server_id: &str, exit_code: i64, out_of_memory: bool) {
    let rt = orch.runtime(server_id);
    let expected = {
        let mut state = rt.state();
        state.stop_tasks();
        state.players = None;
        state.expected_exit.take()
    };
    if let Some(target) = expected {
        orch.set_status(server_id, target).await;
        return;
    }
    let Ok(server) = orch.load(server_id).await else {
        return;
    };
    if matches!(server.status.as_str(), "stopping" | "stopped" | "sleeping") {
        // A stop we initiated already recorded the outcome.
        return;
    }
    if exit_code == 0 && !out_of_memory {
        tracing::info!(server = %server_id, "server stopped on its own");
        orch.set_status(server_id, "stopped").await;
        return;
    }

    orch.set_status(server_id, "crashed").await;
    let reason = if out_of_memory {
        "ran out of memory (raise its memory in Configuration if this keeps happening)".to_owned()
    } else {
        format!("exited with code {exit_code}")
    };
    let allow = server.restart_on_crash && rt.state().allow_crash_restart(Instant::now());
    let message = if allow {
        format!("The server {reason}. Restarting it.")
    } else if server.restart_on_crash {
        format!(
            "The server {reason}. It crashed 3 times in 10 minutes, so it won't be restarted automatically."
        )
    } else {
        format!("The server {reason}.")
    };
    orch.events
        .publish(nn_events::Event::ServerOperationFailed {
            server_id: server_id.to_owned(),
            operation: "crash".into(),
            message,
        });
    if allow {
        let orch = orch.clone();
        let server_id = server_id.to_owned();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(5)).await;
            if let Err(e) = orch.start(&server_id).await {
                orch.report_failure(&server_id, "start", &e);
            }
        });
    }
}

/// Parses Docker's `Exited (137) 2 minutes ago` status text.
fn parse_exit_code(status: &str) -> Option<i64> {
    let start = status.find("Exited (")? + "Exited (".len();
    let end = status[start..].find(')')? + start;
    status[start..end].parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_exit_codes() {
        assert_eq!(parse_exit_code("Exited (137) 2 minutes ago"), Some(137));
        assert_eq!(parse_exit_code("Exited (0) About an hour ago"), Some(0));
        assert_eq!(parse_exit_code("Up 3 hours"), None);
    }
}
