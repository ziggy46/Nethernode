//! Copying a server, and server templates.
//!
//! A copy is made the way a move is (see `moves`): the server is archived
//! where it is, the archive goes to the machine the copy will run on, and
//! that machine sets the copy up from it. Unlike a move, the copy gets its
//! own ID, name, address and ports, and the original stays as it was.
//! Schedules, routes and backups belong to the original and aren't copied.
//!
//! A template is a server's archive without its world, kept beside the
//! panel's own backups (`template-<id>.tar.zst`), with the server's
//! settings. New servers made from it start the same way a copy does.

use connectrpc::ConnectError;
use nn_events::Event;
use nn_node::Node;
use nn_store::{Server, Template, new_id, now_ms};

use crate::AppState;
use crate::backups;
use crate::rpc::store_error;

/// Kind of job, for progress events.
const COPY: &str = "copy";
const TEMPLATE: &str = "template";

/// Where a copy goes.
struct Target {
    id: String,
    name: String,
    node: Node,
}

/// Checks a machine can take part in a copy.
async fn target(state: &AppState, node_id: &str) -> Result<Target, ConnectError> {
    let node_id = match node_id.trim() {
        "" => nn_store::LOCAL_NODE,
        id => id,
    };
    let row = state
        .auth
        .store()
        .find_node(node_id)
        .await
        .map_err(store_error)?
        .ok_or_else(|| ConnectError::not_found("That machine doesn't exist."))?;
    let node = state.nodes.get(node_id);
    ready(state, &node, &row.name)?;
    Ok(Target {
        id: node_id.to_owned(),
        name: row.name,
        node,
    })
}

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

fn checked_name(name: &str) -> Result<String, ConnectError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(ConnectError::invalid_argument("enter a name"));
    }
    if name.chars().count() > 64 {
        return Err(ConnectError::invalid_argument(
            "names can be at most 64 characters",
        ));
    }
    Ok(name.to_owned())
}

fn progress(state: &AppState, server_id: &str, job_id: &str, kind: &str, percent: u8) {
    state.events.publish(Event::FileJobProgress {
        server_id: server_id.to_owned(),
        job_id: job_id.to_owned(),
        kind: kind.to_owned(),
        percent,
    });
}

/// The job a step's progress is reported under.
struct Job<'a> {
    state: &'a AppState,
    server_id: &'a str,
    id: &'a str,
    kind: &'static str,
}

/// The server's world folders: `level-name` from `server.properties`
/// (`world` if unset), with the Nether and End folders some servers keep
/// beside it.
async fn world_folders(state: &AppState, server: &Server) -> Vec<String> {
    let mut level = "world".to_owned();
    if let Ok(folder) = state.nodes.for_server(server).folder(server).await
        && let Ok(path) = nn_files::RelPath::parse("server.properties")
        && let Ok(Some(file)) = folder.read_if_exists(&path, 1024 * 1024).await
    {
        let text = String::from_utf8_lossy(&file.data);
        if let Some(value) = text
            .lines()
            .find_map(|l| l.trim().strip_prefix("level-name="))
        {
            let value = value.trim();
            if !value.is_empty() {
                level = value.to_owned();
            }
        }
    }
    vec![format!("{level}_nether"), format!("{level}_the_end"), level]
}

/// A new server's record, from `source`'s settings.
fn fresh_row(source: &Server, name: &str, node_id: &str) -> Server {
    let now = now_ms();
    Server {
        id: new_id(),
        name: name.to_owned(),
        slug: String::new(),
        status: "installing".into(),
        internal_port: 0,
        public_port: None,
        hostname: None,
        rcon_password_enc: None,
        rcon_port: None,
        container_id: None,
        last_started_at: None,
        mgmt_port: None,
        mgmt_secret_enc: None,
        created_at: now,
        updated_at: now,
        node_id: node_id.to_owned(),
        ..source.clone()
    }
}

/// Moves archive `name` from `from` to `to` (nothing to do on the same
/// machine).
async fn carry(
    from: &Node,
    from_id: &str,
    to: &Target,
    name: &str,
    job: &Job<'_>,
) -> Result<(), String> {
    if from_id == to.id {
        return Ok(());
    }
    let reader = from
        .open_backup(name)
        .await
        .ok_or("the archive disappeared")?;
    let size = reader.size();
    let bytes = reader.from_offset(0).await.map_err(|e| e.to_string())?;
    to.node
        .receive_archive(name, bytes, size)
        .await
        .map_err(|e| format!("couldn't copy it to {}: {e}", to.name))?;
    progress(job.state, job.server_id, job.id, job.kind, 60);
    Ok(())
}

/// Sets up `row` on `to` from archive `name`, with a slug and ports unique
/// across the panel. Returns the new server.
async fn set_up(state: &AppState, row: Server, to: &Target, name: &str) -> Result<Server, String> {
    let store = state.auth.store();
    let _guard = crate::rpc::servers::CREATE_LOCK.lock().await;
    let base = nn_orchestrator::slugify(&row.name);
    let mut slug = base.clone();
    let mut n = 2;
    while store.slug_exists(&slug).await.map_err(|e| e.to_string())? {
        slug = format!("{base}-{n}");
        n += 1;
    }
    let row = Server { slug, ..row };
    let avoid = store.used_ports().await.map_err(|e| e.to_string())?;
    match to.node.adopt(&row, name, &avoid, &new_id()).await {
        Ok(mut made) => {
            if !to.node.is_local() {
                made.node_id = to.id.clone();
                store
                    .upsert_server_mirror(&made, &to.id)
                    .await
                    .map_err(|e| e.to_string())?;
            }
            // Server lists refresh on status events. "installing" (which
            // it just was) doesn't set off start/stop notifications.
            state.events.publish(Event::ServerStatusChanged {
                server_id: made.id.clone(),
                status: "installing".into(),
            });
            if let Err(e) = state.proxy.reload().await {
                tracing::warn!(error = %e, "couldn't reload the proxy after adding a server");
            }
            Ok(made)
        }
        Err(e) => {
            let _ = to.node.release(&row).await;
            if to.node.is_local() {
                let _ = store.delete_server(&row.id).await;
            }
            Err(e)
        }
    }
}

/// Starts copying `source` to machine `node_id` as `name`. Returns the job
/// ID its progress and outcome are reported under.
pub(crate) async fn start_copy(
    state: &AppState,
    source: Server,
    name: &str,
    node_id: &str,
    with_world: bool,
) -> Result<String, ConnectError> {
    let name = checked_name(name)?;
    crate::moves::not_moving(state, &source.id)?;
    let from = state.nodes.for_server(&source);
    ready(state, &from, &source.name)?;
    let to = target(state, node_id).await?;
    let job_id = new_id();
    let (state, job) = (state.clone(), job_id.clone());
    tokio::spawn(async move {
        let result = copy(&state, &source, &from, &to, &name, with_world, &job).await;
        match &result {
            Ok(message) => tracing::info!(server = %source.id, "{message}"),
            Err(message) => tracing::warn!(server = %source.id, "{message}"),
        }
        backups::finished(&state, &source.id, &job, COPY, &result);
    });
    Ok(job_id)
}

async fn copy(
    state: &AppState,
    source: &Server,
    from: &Node,
    to: &Target,
    name: &str,
    with_world: bool,
    job_id: &str,
) -> Result<String, String> {
    progress(state, &source.id, job_id, COPY, 1);
    let leave_out = if with_world {
        Vec::new()
    } else {
        world_folders(state, source).await
    };
    let archive = format!("copy-{job_id}.tar.zst");
    from.backup_without(source, &archive, &new_id(), &leave_out)
        .await
        .map_err(|e| format!("Couldn't copy {}: {e}", source.name))?;
    progress(state, &source.id, job_id, COPY, 30);
    let result = async {
        let job = Job {
            state,
            server_id: &source.id,
            id: job_id,
            kind: COPY,
        };
        carry(from, &source.node_id, to, &archive, &job).await?;
        set_up(state, fresh_row(source, name, &to.id), to, &archive).await
    }
    .await;
    // The archive was only for the copy.
    let _ = from.delete_backup(&archive).await;
    if source.node_id != to.id {
        let _ = to.node.delete_backup(&archive).await;
    }
    let made = result.map_err(|e| format!("Couldn't copy {}: {e}", source.name))?;
    Ok(format!(
        "Copied {} to {}{}. It's stopped; start it when you're ready.",
        source.name,
        made.name,
        if to.node.is_local() && from.is_local() {
            String::new()
        } else {
            format!(" on {}", to.name)
        },
    ))
}

/// Starts saving `source`, without its world, as template `name`.
pub(crate) async fn start_save_template(
    state: &AppState,
    source: Server,
    name: &str,
) -> Result<String, ConnectError> {
    let name = checked_name(name)?;
    crate::moves::not_moving(state, &source.id)?;
    let from = state.nodes.for_server(&source);
    ready(state, &from, &source.name)?;
    let job_id = new_id();
    let (state, job) = (state.clone(), job_id.clone());
    tokio::spawn(async move {
        let result = save_template(&state, &source, &from, &name, &job).await;
        if let Err(message) = &result {
            tracing::warn!(server = %source.id, "{message}");
        }
        backups::finished(&state, &source.id, &job, TEMPLATE, &result);
    });
    Ok(job_id)
}

async fn save_template(
    state: &AppState,
    source: &Server,
    from: &Node,
    name: &str,
    job_id: &str,
) -> Result<String, String> {
    progress(state, &source.id, job_id, TEMPLATE, 1);
    let id = new_id();
    let archive = nn_store::templates::archive_name(&id);
    let leave_out = world_folders(state, source).await;
    let failed = |e: String| format!("Couldn't save {} as a template: {e}", source.name);
    let made = from
        .backup_without(source, &archive, &new_id(), &leave_out)
        .await
        .map_err(failed)?;
    progress(state, &source.id, job_id, TEMPLATE, 50);
    let panel = Target {
        id: nn_store::LOCAL_NODE.into(),
        name: "this machine".into(),
        node: state.nodes.local().clone(),
    };
    let job = Job {
        state,
        server_id: &source.id,
        id: job_id,
        kind: TEMPLATE,
    };
    let carried = carry(from, &source.node_id, &panel, &archive, &job).await;
    if source.node_id != nn_store::LOCAL_NODE {
        let _ = from.delete_backup(&archive).await;
    }
    carried.map_err(failed)?;
    let template = Template {
        id,
        name: name.to_owned(),
        source_name: source.name.clone(),
        loader: source.loader.clone(),
        mc_version: source.mc_version.clone(),
        memory_mb: source.memory_mb,
        server_json: serde_json::to_string(source).map_err(|e| failed(e.to_string()))?,
        archive_bytes: i64::try_from(made.size).unwrap_or(i64::MAX),
        created_at: now_ms(),
    };
    if let Err(e) = state.auth.store().insert_template(&template).await {
        let _ = panel.node.delete_backup(&archive).await;
        return Err(failed(e.to_string()));
    }
    state.events.publish(Event::TemplatesChanged);
    Ok(format!("Saved {} as the template {name}.", source.name))
}

/// Starts creating server `name` on machine `node_id` from `template`.
pub(crate) async fn start_from_template(
    state: &AppState,
    template: Template,
    name: &str,
    node_id: &str,
) -> Result<String, ConnectError> {
    let name = checked_name(name)?;
    let source: Server = serde_json::from_str(&template.server_json)
        .map_err(|_| ConnectError::failed_precondition("That template is damaged."))?;
    let to = target(state, node_id).await?;
    let row = fresh_row(&source, &name, &to.id);
    let job_id = new_id();
    let (state, job, server_id) = (state.clone(), job_id.clone(), row.id.clone());
    tokio::spawn(async move {
        let result = from_template(&state, &template, row, &to, &job).await;
        if let Err(message) = &result {
            tracing::warn!(template = %template.id, "{message}");
        }
        backups::finished(&state, &server_id, &job, TEMPLATE, &result);
    });
    Ok(job_id)
}

async fn from_template(
    state: &AppState,
    template: &Template,
    row: Server,
    to: &Target,
    job_id: &str,
) -> Result<String, String> {
    progress(state, &row.id, job_id, TEMPLATE, 1);
    let panel = state.nodes.local().clone();
    let stored = template.archive();
    // The machine gets its own copy to set the server up from.
    let archive = format!("copy-{job_id}.tar.zst");
    let name = row.name.clone();
    let failed = |e: String| format!("Couldn't create {name} from {}: {e}", template.name);
    let reader = panel
        .open_backup(&stored)
        .await
        .ok_or_else(|| failed("its archive is missing".into()))?;
    let size = reader.size();
    let bytes = reader
        .from_offset(0)
        .await
        .map_err(|e| failed(e.to_string()))?;
    to.node
        .receive_archive(&archive, bytes, size)
        .await
        .map_err(failed)?;
    progress(state, &row.id, job_id, TEMPLATE, 50);
    let made = set_up(state, row, to, &archive).await;
    let _ = to.node.delete_backup(&archive).await;
    made.map_err(failed)?;
    Ok(format!(
        "Created {name} from {}. It's stopped; start it when you're ready.",
        template.name
    ))
}

/// Deletes a template and its archive.
pub(crate) async fn delete_template(state: &AppState, id: &str) -> Result<Template, ConnectError> {
    let store = state.auth.store();
    let template = store
        .find_template(id)
        .await
        .map_err(store_error)?
        .ok_or_else(|| ConnectError::not_found("That template doesn't exist."))?;
    store.delete_template(id).await.map_err(store_error)?;
    if let Err(e) = state.nodes.local().delete_backup(&template.archive()).await {
        tracing::warn!(template = %id, error = %e, "couldn't delete a template's archive");
    }
    state.events.publish(Event::TemplatesChanged);
    Ok(template)
}
