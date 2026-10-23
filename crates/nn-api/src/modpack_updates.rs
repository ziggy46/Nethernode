//! Moving modpack servers to newer versions of their packs, and noticing
//! when there is one.
//!
//! The server image installs the pack as the container starts, from the
//! version pinned in the server's settings (and cleans up what the old
//! version installed). An update stops the server, backs it up, pins the new
//! version, and starts it again. Every backup of a modpack server records
//! its pack version, so restoring the one made first puts the old version
//! back. If an update can't go ahead, the server is started again on the
//! version it had. Newer versions and failed updates show under the bell
//! and go out as notifications (see `notify`).

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use connectrpc::ConnectError;
use nn_catalog::{ProjectVersion, compare_versions};
use nn_events::Event;
use nn_orchestrator::spec::{MODPACK_LOADERS, Modpack};
use nn_store::{Backup, Server, ServerSettings};

use crate::AppState;
use crate::backups;
use crate::content::catalog_error;
use crate::content::modpacks::newest_game_version;

/// How often packs are checked for newer versions (while checking for
/// updates is on).
const CHECK_EVERY: Duration = Duration::from_secs(24 * 60 * 60);
/// Most newer versions listed.
const MAX_NEWER: usize = 10;
/// How long a failed update stays under the bell.
const FAILURE_SHOWN_MS: i64 = 24 * 60 * 60 * 1000;
/// Which newer version was announced for each server (server ID → version
/// ID), so each is announced once.
const ANNOUNCED_SETTING: &str = "modpacks.announced";
const NO_CURSEFORGE_KEY: &str =
    "Add a CurseForge API key in Settings to look up newer versions of this pack.";

/// What checking a server's pack found.
pub(crate) struct Check {
    pub pack: Modpack,
    /// The installed version, when the platform still lists it.
    pub current: Option<ProjectVersion>,
    /// Newer versions that can be installed, newest first.
    pub newer: Vec<ProjectVersion>,
    /// Not pinned to a version: the newest installs each time it starts.
    pub follows_newest: bool,
    /// Why newer versions can't be looked up.
    pub unavailable: Option<String>,
}

/// The newest version last found for a server's pack.
#[derive(Debug, Clone)]
struct Found {
    /// The version installed when it was found.
    installed: String,
    version_name: String,
}

static FOUND: LazyLock<Mutex<HashMap<String, Found>>> = LazyLock::new(Default::default);

fn found() -> std::sync::MutexGuard<'static, HashMap<String, Found>> {
    FOUND.lock().unwrap_or_else(|e| e.into_inner())
}

/// An update that went wrong.
#[derive(Debug, Clone)]
pub(crate) struct Failure {
    pub message: String,
    /// Unix ms.
    pub at_ms: i64,
}

static FAILED: LazyLock<Mutex<HashMap<String, Failure>>> = LazyLock::new(Default::default);

fn failed() -> std::sync::MutexGuard<'static, HashMap<String, Failure>> {
    FAILED.lock().unwrap_or_else(|e| e.into_inner())
}

/// A server's last pack update, if it failed in the last day.
pub(crate) fn failure(server_id: &str) -> Option<Failure> {
    let failure = failed().get(server_id).cloned()?;
    (nn_store::now_ms() - failure.at_ms < FAILURE_SHOWN_MS).then_some(failure)
}

/// The modpack a server is installed from, if it is one.
pub(crate) fn pack_of(server: &Server) -> Option<Modpack> {
    if !MODPACK_LOADERS.contains(&server.loader.as_str()) {
        return None;
    }
    serde_json::from_str(server.modpack_json.as_deref()?).ok()
}

/// The pinned version, if any.
fn installed(pack: &Modpack) -> Option<&str> {
    pack.version_id.as_deref().filter(|v| !v.is_empty())
}

/// The CurseForge key, when the pack is on CurseForge and can be looked
/// up. `Ok(None)` for Modrinth.
fn curseforge_key(state: &AppState, pack: &Modpack) -> Result<Option<String>, String> {
    if pack.platform != "curseforge" {
        return Ok(None);
    }
    match state.content.curseforge_key() {
        Some(key) if !pack.project_id.is_empty() => Ok(Some(key)),
        Some(_) => Err(
            "This pack was added before there was a CurseForge API key, so its versions can't be looked up."
                .to_owned(),
        ),
        None => Err(NO_CURSEFORGE_KEY.to_owned()),
    }
}

fn project(pack: &Modpack) -> &str {
    if pack.project_id.is_empty() {
        &pack.slug
    } else {
        &pack.project_id
    }
}

async fn versions(state: &AppState, pack: &Modpack) -> Result<Vec<ProjectVersion>, ConnectError> {
    let catalog = &state.content.catalog;
    match curseforge_key(state, pack).map_err(ConnectError::failed_precondition)? {
        Some(key) => {
            catalog
                .curseforge_versions(&key, project(pack), None, None)
                .await
        }
        None => catalog.modrinth_versions(project(pack), None, None).await,
    }
    .map_err(catalog_error)
}

async fn version(
    state: &AppState,
    pack: &Modpack,
    id: &str,
) -> Result<ProjectVersion, ConnectError> {
    let catalog = &state.content.catalog;
    let version = match curseforge_key(state, pack).map_err(ConnectError::failed_precondition)? {
        Some(key) => catalog.curseforge_version(&key, project(pack), id).await,
        None => catalog.modrinth_version(id).await,
    }
    .map_err(catalog_error)?;
    if pack.platform == "modrinth" && version.project_id != pack.project_id {
        return Err(ConnectError::invalid_argument(
            "that version belongs to another project",
        ));
    }
    Ok(version)
}

/// Why `target` can't replace `current` on a server running `mc_version`,
/// if it can't.
fn refusal(
    mc_version: &str,
    current: Option<&ProjectVersion>,
    target: &ProjectVersion,
) -> Option<String> {
    if let Some(current) = current
        && target.published_ms <= current.published_ms
    {
        return Some(
            "That isn't newer than the installed version. To go back, restore a backup made before the update."
                .into(),
        );
    }
    let Some(target_mc) = newest_game_version(target) else {
        return Some("That version doesn't say which Minecraft version it's for.".into());
    };
    let pinned = mc_version.starts_with(|c: char| c.is_ascii_digit());
    if pinned && compare_versions(&target_mc, mc_version).is_lt() {
        return Some(format!(
            "That version is for Minecraft {target_mc}, older than this server's {mc_version}. Worlds can't go back to an older Minecraft."
        ));
    }
    None
}

/// The versions in `all` that could replace the installed one, newest
/// first.
fn newer_versions(
    all: Vec<ProjectVersion>,
    installed: &str,
    current: Option<&ProjectVersion>,
    mc_version: &str,
) -> Vec<ProjectVersion> {
    let mut newer: Vec<ProjectVersion> = all
        .into_iter()
        .filter(|v| v.id != installed && refusal(mc_version, current, v).is_none())
        .collect();
    newer.sort_by_key(|v| std::cmp::Reverse(v.published_ms));
    newer.truncate(MAX_NEWER);
    newer
}

/// Looks up a server's pack and its newer versions, and remembers the
/// newest for the bell.
pub(crate) async fn check(state: &AppState, server: &Server) -> Result<Check, ConnectError> {
    let pack = pack_of(server).ok_or_else(|| {
        ConnectError::failed_precondition(format!("{} isn't installed from a modpack", server.name))
    })?;
    let mut out = Check {
        pack: pack.clone(),
        current: None,
        newer: Vec::new(),
        follows_newest: false,
        unavailable: None,
    };
    let Some(installed) = installed(&pack) else {
        out.follows_newest = true;
        return Ok(out);
    };
    if let Err(reason) = curseforge_key(state, &pack) {
        out.unavailable = Some(reason);
        return Ok(out);
    }
    let all = versions(state, &pack).await?;
    out.current = match all.iter().find(|v| v.id == installed) {
        Some(v) => Some(v.clone()),
        None => version(state, &pack, installed).await.ok(),
    };
    out.newer = newer_versions(all, installed, out.current.as_ref(), &server.mc_version);
    match out.newer.first() {
        // Only when the installed version is known to be older.
        Some(newest) if out.current.is_some() => {
            found().insert(
                server.id.clone(),
                Found {
                    installed: installed.to_owned(),
                    version_name: newest.version_number.clone(),
                },
            );
            announce(state, server, &pack, newest).await;
        }
        _ => {
            found().remove(&server.id);
        }
    }
    Ok(out)
}

/// Tells notification channels about a newer version, once per version.
async fn announce(state: &AppState, server: &Server, pack: &Modpack, newest: &ProjectVersion) {
    let store = state.auth.store();
    let mut announced: HashMap<String, String> = store
        .get_setting(ANNOUNCED_SETTING)
        .await
        .ok()
        .flatten()
        .unwrap_or_default();
    if announced.get(&server.id) == Some(&newest.id) {
        return;
    }
    announced.insert(server.id.clone(), newest.id.clone());
    if let Err(e) = store.set_setting(ANNOUNCED_SETTING, &announced).await {
        tracing::warn!(error = %e, "couldn't record an announced modpack version");
        return;
    }
    state.events.publish(Event::ModpackUpdateAvailable {
        server_id: server.id.clone(),
        pack: pack.title.clone(),
        version: newest.version_number.clone(),
    });
}

/// The newer version's name, if the last check found one for the version
/// still installed.
pub(crate) fn available(server: &Server) -> Option<String> {
    let pack = pack_of(server)?;
    let found = found().get(&server.id).cloned()?;
    (installed(&pack) == Some(found.installed.as_str())).then_some(found.version_name)
}

/// Checks every modpack server's pack about once a day, while checking for
/// updates is on.
pub fn spawn_checks(state: AppState) {
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(5 * 60)).await;
        loop {
            if crate::notices::update_check_on(&state).await {
                for server in state.auth.store().list_servers().await.unwrap_or_default() {
                    if pack_of(&server).is_none() {
                        continue;
                    }
                    if let Err(e) = check(&state, &server).await {
                        tracing::debug!(server = %server.id, error = %e, "couldn't check a modpack for updates");
                    }
                    tokio::time::sleep(Duration::from_secs(2)).await;
                }
            } else {
                found().clear();
            }
            tokio::time::sleep(CHECK_EVERY).await;
        }
    });
}

/// What an update does, worked out before it starts.
pub(crate) struct Plan {
    pub from: Modpack,
    pub to: Modpack,
    pub mc_version: String,
}

/// Checks that `version_id` can replace a server's pack version.
pub(crate) async fn plan(
    state: &AppState,
    server: &Server,
    version_id: &str,
) -> Result<Plan, ConnectError> {
    let from = pack_of(server).ok_or_else(|| {
        ConnectError::failed_precondition(format!("{} isn't installed from a modpack", server.name))
    })?;
    let version_id = version_id.trim();
    if version_id.is_empty() {
        return Err(ConnectError::invalid_argument("choose a version"));
    }
    if installed(&from) == Some(version_id) {
        return Err(ConnectError::failed_precondition(
            "that version is already installed",
        ));
    }
    let target = version(state, &from, version_id).await?;
    let current = match installed(&from) {
        Some(id) => version(state, &from, id).await.ok(),
        None => None,
    };
    if let Some(reason) = refusal(&server.mc_version, current.as_ref(), &target) {
        return Err(ConnectError::failed_precondition(reason));
    }
    let content = &state.content;
    let resolved = if from.platform == "curseforge" {
        content
            .resolve_curseforge(project(&from), Some(version_id), None)
            .await?
    } else {
        content
            .resolve_modrinth(project(&from), Some(version_id))
            .await?
    };
    Ok(Plan {
        from,
        to: resolved.modpack,
        mc_version: resolved.mc_version,
    })
}

fn version_label(pack: &Modpack) -> String {
    pack.version_name
        .clone()
        .or_else(|| pack.version_id.clone())
        .unwrap_or_else(|| "the newest version".into())
}

/// Starts updating a server's pack in the background. Returns the job ID.
pub(crate) fn start(
    state: &AppState,
    server: Server,
    plan: Plan,
    user_id: &str,
    warning: Option<crate::countdown::Ticket>,
) -> String {
    let job_id = nn_store::new_id();
    let (state, job, user) = (state.clone(), job_id.clone(), user_id.to_owned());
    tokio::spawn(async move {
        // Players are warned first; the server is looked at again afterwards,
        // since it may have stopped meanwhile.
        let mut server = server;
        if let Some(ticket) = warning {
            if !ticket.wait(&state).await {
                let message = Ok(format!(
                    "The update of {} was called off; nothing changed.",
                    server.name
                ));
                // Its own kind, so it isn't reported as an update.
                backups::finished(
                    &state,
                    &server.id,
                    &job,
                    "modpack-update-called-off",
                    &message,
                );
                return;
            }
            if let Ok(Some(now)) = state.auth.store().find_server(&server.id).await {
                server = now;
            }
        }
        let result = run(&state, &server, plan, &user).await;
        match &result {
            Ok(_) => {
                failed().remove(&server.id);
            }
            Err(message) => {
                failed().insert(
                    server.id.clone(),
                    Failure {
                        message: message.clone(),
                        at_ms: nn_store::now_ms(),
                    },
                );
            }
        }
        backups::finished(&state, &server.id, &job, "modpack-update", &result);
    });
    job_id
}

/// `text` ending in a full stop.
fn sentence(text: &str) -> String {
    let text = text.trim_end();
    if text.ends_with(['.', '!', '?']) {
        text.to_owned()
    } else {
        format!("{text}.")
    }
}

async fn run(
    state: &AppState,
    server: &Server,
    plan: Plan,
    user_id: &str,
) -> Result<String, String> {
    let node = state.nodes.for_server(server);
    let was_running = matches!(server.status.as_str(), "running" | "starting");
    if !matches!(server.status.as_str(), "stopped" | "crashed") {
        node.stop(&server.id, false).await.map_err(|e| {
            format!(
                "{} couldn't be stopped, so its pack wasn't updated: {e}",
                server.name
            )
        })?;
    }
    let (from, to) = (version_label(&plan.from), version_label(&plan.to));
    if let Err(e) = switch(state, server, &plan, user_id, &from, &to).await {
        // Nothing changed: back to how it was.
        let mut message = format!("{} The pack wasn't updated.", sentence(&e));
        if was_running {
            match node.start(&server.id).await {
                Ok(()) => {
                    message.push_str(&format!(" {} was started again on {from}.", server.name))
                }
                Err(e) => {
                    message.push_str(&format!(" {} couldn't be started again: {e}", server.name))
                }
            }
        }
        return Err(message);
    }
    found().remove(&server.id);
    if was_running && let Err(e) = node.start(&server.id).await {
        return Err(format!(
            "{} was switched to {to}, but it didn't start: {e} To go back, restore the backup made before the update.",
            server.name
        ));
    }
    Ok(format!(
        "{} moves from {from} to {to} as it starts. A backup of the old version was made first.",
        server.name
    ))
}

/// Backs the server up, then pins the new version. Nothing is changed if
/// either fails.
async fn switch(
    state: &AppState,
    server: &Server,
    plan: &Plan,
    user_id: &str,
    from: &str,
    to: &str,
) -> Result<(), String> {
    let label = format!("Before updating {} from {from} to {to}", plan.from.title);
    backups::labelled_backup_and_wait(state, server, user_id, &label).await?;
    let current = state
        .auth
        .store()
        .find_server(&server.id)
        .await
        .map_err(|e| e.to_string())?
        .ok_or("The server was deleted.")?;
    // Each way of installing removes only what it put there itself, so
    // moving between the modpack and its server pack starts the mods over
    // (the backup above keeps the old ones).
    let server_pack = |p: &Modpack| p.server_pack_install().is_some();
    if server_pack(&plan.from) != server_pack(&plan.to) {
        clear_mods(state, &current)
            .await
            .map_err(|e| format!("The old mods couldn't be cleared out: {e}"))?;
    }
    let mut settings = current.settings();
    settings.modpack_json = Some(serde_json::to_string(&plan.to).map_err(|e| e.to_string())?);
    settings.mc_version = plan.mc_version.clone();
    apply(state, &current, settings)
        .await
        .map_err(|e| format!("The new version couldn't be saved: {e}"))?;
    Ok(())
}

/// Moves the mods (`mods/*.jar`) to the trash.
async fn clear_mods(state: &AppState, server: &Server) -> Result<(), String> {
    let folder = state
        .nodes
        .for_server(server)
        .folder(server)
        .await
        .map_err(|e| e.to_string())?;
    let mods = nn_files::RelPath::parse("mods").map_err(|e| e.to_string())?;
    let jars: Vec<nn_files::RelPath> = match folder.list(&mods).await {
        Ok(entries) => entries
            .into_iter()
            .filter(|e| e.kind == nn_files::EntryKind::File && e.name.ends_with(".jar"))
            .filter_map(|e| nn_files::RelPath::parse(&format!("mods/{}", e.name)).ok())
            .collect(),
        // No mods folder yet: nothing to clear.
        Err(_) => return Ok(()),
    };
    if !jars.is_empty() {
        folder
            .delete(jars, nn_files::TRASH_RETENTION)
            .await
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Saves a server's settings on its machine (and the panel's copy for
/// servers on other machines).
async fn apply(
    state: &AppState,
    server: &Server,
    settings: ServerSettings,
) -> Result<Server, String> {
    let node = state.nodes.for_server(server);
    let (mut updated, _) = node
        .update(&server.id, settings)
        .await
        .map_err(|e| e.to_string())?;
    if !node.is_local() {
        updated.node_id = server.node_id.clone();
        state
            .auth
            .store()
            .upsert_server_mirror(&updated, &server.node_id)
            .await
            .map_err(|e| e.to_string())?;
    }
    Ok(updated)
}

/// The pack (JSON) and Minecraft version to put back when restoring a
/// backup, when they differ from the server's now. Only the same pack.
fn pack_to_restore<'a>(
    server_pack: Option<&str>,
    server_mc: &str,
    backup_pack: Option<&'a str>,
    backup_mc: Option<&'a str>,
) -> Option<(&'a str, Option<&'a str>)> {
    let json = backup_pack?;
    let old: Modpack = serde_json::from_str(json).ok()?;
    let now: Modpack = serde_json::from_str(server_pack?).ok()?;
    if old.platform != now.platform || project(&old) != project(&now) {
        return None;
    }
    let same_mc = backup_mc.is_none_or(|v| v == server_mc);
    if old.version_id == now.version_id && same_mc {
        return None;
    }
    Some((json, backup_mc))
}

/// After a backup's files are restored: puts its pack version back.
/// Returns a sentence to add to the result, if anything changed or failed.
pub(crate) async fn restore_pack(
    state: &AppState,
    server_id: &str,
    backup: &Backup,
    docker: bool,
) -> Option<String> {
    let server = state.auth.store().find_server(server_id).await.ok()??;
    let (json, mc) = pack_to_restore(
        server.modpack_json.as_deref(),
        &server.mc_version,
        backup.modpack_json.as_deref(),
        backup.mc_version.as_deref(),
    )?;
    let old: Modpack = serde_json::from_str(json).ok()?;
    if !docker {
        return Some(format!(
            " Its pack couldn't be put back to {} because Docker isn't available.",
            version_label(&old)
        ));
    }
    let mut settings = server.settings();
    settings.modpack_json = Some(json.to_owned());
    if let Some(mc) = mc {
        settings.mc_version = mc.to_owned();
    }
    found().remove(&server.id);
    Some(match apply(state, &server, settings).await {
        Ok(_) => format!(" Its pack is back on {}.", version_label(&old)),
        Err(e) => format!(
            " Its pack couldn't be put back to {}: {e}",
            version_label(&old)
        ),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn version(id: &str, published_ms: i64, mc: &str) -> ProjectVersion {
        ProjectVersion {
            id: id.into(),
            project_id: "P".into(),
            name: id.into(),
            version_number: id.into(),
            game_versions: vec![mc.into()],
            loaders: vec!["fabric".into()],
            channel: "release".into(),
            published_ms,
            files: Vec::new(),
            dependencies: Vec::new(),
            server_pack_id: None,
        }
    }

    #[test]
    fn only_newer_versions_for_the_same_or_newer_minecraft() {
        let current = version("v2", 200, "1.21.1");
        assert!(refusal("1.21.1", Some(&current), &version("v3", 300, "1.21.1")).is_none());
        assert!(refusal("1.21.1", Some(&current), &version("v4", 400, "1.21.4")).is_none());
        let older = refusal("1.21.1", Some(&current), &version("v1", 100, "1.21.1")).unwrap();
        assert!(older.contains("restore a backup"), "{older}");
        let downgrade = refusal("1.21.1", Some(&current), &version("v5", 500, "1.20.1")).unwrap();
        assert!(downgrade.contains("older Minecraft"), "{downgrade}");
        // Unknown installed version: only Minecraft is checked.
        assert!(refusal("1.21.1", None, &version("v0", 1, "1.21.1")).is_none());

        let all = vec![
            version("v1", 100, "1.21.1"),
            current.clone(),
            version("v3", 300, "1.21.1"),
            version("v5", 500, "1.20.1"),
            version("v4", 400, "1.21.4"),
        ];
        let newer = newer_versions(all, "v2", Some(&current), "1.21.1");
        let ids: Vec<&str> = newer.iter().map(|v| v.id.as_str()).collect();
        assert_eq!(ids, ["v4", "v3"]);
    }

    fn pack(version: &str) -> String {
        serde_json::to_string(&Modpack {
            platform: "modrinth".into(),
            project_id: "P".into(),
            slug: "pack".into(),
            title: "Pack".into(),
            version_id: Some(version.into()),
            version_name: Some(version.into()),
            loader: Some("fabric".into()),
            icon_url: None,
            page_url: None,
            ..Default::default()
        })
        .unwrap()
    }

    #[test]
    fn messages_end_in_one_full_stop() {
        assert_eq!(
            sentence("The backup failed: disk full"),
            "The backup failed: disk full."
        );
        assert_eq!(sentence("Done. "), "Done.");
    }

    #[test]
    fn restoring_puts_the_pack_version_back() {
        let (now, then) = (pack("v3"), pack("v2"));
        assert_eq!(
            pack_to_restore(Some(&now), "1.21.4", Some(&then), Some("1.21.1")),
            Some((then.as_str(), Some("1.21.1")))
        );
        // Same version: nothing to do.
        assert_eq!(
            pack_to_restore(Some(&now), "1.21.4", Some(&now), Some("1.21.4")),
            None
        );
        // Backups from before snapshots, and of servers that aren't packs.
        assert_eq!(pack_to_restore(Some(&now), "1.21.4", None, None), None);
        assert_eq!(pack_to_restore(None, "1.21.4", Some(&then), None), None);
        // Another pack: left alone.
        let other = now.replace("\"P\"", "\"Q\"");
        assert_eq!(
            pack_to_restore(Some(&other), "1.21.4", Some(&then), Some("1.21.1")),
            None
        );
    }
}
