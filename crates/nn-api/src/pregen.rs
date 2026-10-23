//! Pre-generating the world around its spawn with Chunky (a mod, or a
//! plugin on Paper and Purpur), so players exploring don't wait for chunks
//! to generate. NetherNode installs Chunky from Modrinth and drives it with
//! console commands; vanilla servers can't run it.

use nn_files::{EntryKind, Expect, RelPath};
use nn_node::{Folder, Node};
use nn_store::Server;

use crate::AppState;

/// Chunky on Modrinth.
const CHUNKY: &str = "chunky";
/// The largest Chunky jar accepted.
const MAX_JAR: u64 = 32 * 1024 * 1024;

/// Where Chunky can run, and which build of it.
pub(crate) enum Support {
    /// It can't (vanilla).
    No(&'static str),
    /// A mod for this loader, in mods/.
    Mod(String),
    /// A plugin, in plugins/.
    Plugin(&'static str),
}

/// What a server runs Chunky as.
pub(crate) fn support(server: &Server) -> Support {
    let loader = match server.loader.as_str() {
        "modrinth" | "curseforge" => crate::modpack_updates::pack_of(server)
            .and_then(|p| p.loader)
            .unwrap_or_default(),
        other => other.to_owned(),
    };
    match loader.as_str() {
        "fabric" | "quilt" | "forge" | "neoforge" => Support::Mod(loader),
        "paper" | "purpur" => Support::Plugin("paper"),
        "" => {
            Support::No("NetherNode doesn't know this pack's mod loader, so it can't add Chunky.")
        }
        _ => Support::No(
            "Pre-generating needs Chunky, a mod or plugin; vanilla servers can't run it.",
        ),
    }
}

/// The folder Chunky goes in.
fn folder_for(support: &Support) -> Option<&'static str> {
    match support {
        Support::Mod(_) => Some("mods"),
        Support::Plugin(_) => Some("plugins"),
        Support::No(_) => None,
    }
}

/// Whether Chunky is in the server's mods or plugins (and turned on).
pub(crate) async fn installed(folder: &Folder, server: &Server) -> bool {
    let Some(dir) = folder_for(&support(server)) else {
        return false;
    };
    let Ok(path) = RelPath::parse(dir) else {
        return false;
    };
    folder.list(&path).await.is_ok_and(|entries| {
        entries.iter().any(|e| {
            e.kind == EntryKind::File
                && e.name.to_ascii_lowercase().starts_with("chunky")
                && e.name.ends_with(".jar")
        })
    })
}

/// Downloads the Chunky build for this server's loader and Minecraft
/// version from Modrinth. It loads the next time the server starts.
pub(crate) async fn install(
    state: &AppState,
    server: &Server,
    folder: &Folder,
) -> Result<String, String> {
    let support = support(server);
    let (dir, loader) = match &support {
        Support::No(why) => return Err((*why).to_owned()),
        Support::Mod(loader) => ("mods", loader.as_str()),
        Support::Plugin(loader) => ("plugins", *loader),
    };
    let catalog = &state.content.catalog;
    let versions = catalog
        .modrinth_versions(CHUNKY, Some(&server.mc_version), Some(loader))
        .await
        .map_err(|e| format!("Modrinth couldn't be asked for Chunky: {e}"))?;
    let version = versions
        .iter()
        .find(|v| v.channel == "release")
        .or_else(|| versions.first())
        .ok_or_else(|| {
            format!(
                "Chunky has no build for Minecraft {} with {loader}.",
                server.mc_version
            )
        })?;
    let file = version
        .files
        .iter()
        .find(|f| f.primary)
        .or_else(|| version.files.first())
        .ok_or("Chunky's download is missing.")?;
    let bytes = catalog
        .download(file, MAX_JAR)
        .await
        .map_err(|e| format!("Chunky couldn't be downloaded: {e}"))?;
    let path = RelPath::parse(&format!("{dir}/{}", file.filename)).map_err(|e| e.to_string())?;
    let _ = folder
        .mkdir(&RelPath::parse(dir).map_err(|e| e.to_string())?)
        .await;
    folder
        .write(&path, bytes, Expect::Any)
        .await
        .map_err(|e| e.to_string())?;
    Ok(format!(
        "Chunky {} is in {dir}/. Restart the server to load it.",
        version.version_number
    ))
}

/// A pre-generation task's progress, from Chunky's own report.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Progress {
    pub running: bool,
    pub percent: f64,
    pub chunks: u64,
    /// "h:mm:ss".
    pub eta: String,
    /// Chunky's words, for anything else.
    pub message: String,
}

/// Reads Chunky's `progress` reply: "Task running for minecraft:overworld.
/// Processed: 1234 chunks (5.67%), ETA: 0:12:34, Rate: 45.6 cps, Current:
/// 12, 34", or "No tasks running.".
pub(crate) fn parse_progress(reply: &str) -> Progress {
    let text = strip_codes(reply);
    let line = text
        .lines()
        .find(|l| l.contains("Task running") || l.contains("Task finished"))
        .unwrap_or_else(|| text.trim());
    let number = |after: &str, until: &str| -> Option<String> {
        let rest = line.split(after).nth(1)?;
        Some(rest.split(until).next()?.trim().to_owned())
    };
    Progress {
        running: line.contains("Task running"),
        percent: number("(", "%").and_then(|p| p.parse().ok()).unwrap_or(0.0),
        chunks: number("Processed: ", " chunks")
            .and_then(|c| c.replace(',', "").parse().ok())
            .unwrap_or(0),
        eta: number("ETA: ", ",").unwrap_or_default(),
        message: line.trim().to_owned(),
    }
}

/// Minecraft's `§x` colour codes and Chunky's `[Chunky]` prefix out.
fn strip_codes(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c == '§' {
            chars.next();
        } else {
            out.push(c);
        }
    }
    out.replace("[Chunky] ", "")
}

/// Chunky's progress, when the server runs and Chunky is there.
pub(crate) async fn progress(node: &Node, server: &Server) -> Option<Progress> {
    if server.status != "running" {
        return None;
    }
    let reply = node.command(&server.id, "chunky progress").await.ok()?;
    if reply.to_ascii_lowercase().contains("unknown") {
        return None;
    }
    Some(parse_progress(&reply))
}

/// Starts pre-generating `radius` blocks around the world spawn. Returns
/// Chunky's reply (which may ask to confirm).
pub(crate) async fn start(
    node: &Node,
    server: &Server,
    radius: u32,
    confirm: bool,
) -> Result<String, String> {
    if server.status != "running" {
        return Err("Start the server first: Chunky runs inside it.".into());
    }
    let run = async |command: &str| -> Result<String, String> {
        node.command(&server.id, command)
            .await
            .map(|r| strip_codes(&r).trim().to_owned())
            .map_err(|e| e.to_string())
    };
    if confirm {
        return run("chunky confirm").await;
    }
    run("chunky spawn").await?;
    run(&format!("chunky radius {}", radius.clamp(16, 100_000))).await?;
    run("chunky start").await
}

/// Stops the running task (it can't be continued afterwards).
pub(crate) async fn cancel(node: &Node, server: &Server) -> Result<String, String> {
    let first = node
        .command(&server.id, "chunky cancel")
        .await
        .map_err(|e| e.to_string())?;
    // Chunky asks to confirm a cancel.
    if first.contains("chunky confirm") {
        return node
            .command(&server.id, "chunky confirm")
            .await
            .map(|r| strip_codes(&r).trim().to_owned())
            .map_err(|e| e.to_string());
    }
    Ok(strip_codes(&first).trim().to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunkys_progress_is_read() {
        let p = parse_progress(
            "§2[Chunky]§r Task running for minecraft:overworld. Processed: 1234 chunks (5.67%), ETA: 0:12:34, Rate: 45.6 cps, Current: 12, 34",
        );
        assert!(p.running);
        assert_eq!((p.chunks, p.eta.as_str()), (1234, "0:12:34"));
        assert!((p.percent - 5.67).abs() < 1e-9);
        let idle = parse_progress("[Chunky] No tasks running.");
        assert!(!idle.running);
        assert_eq!(idle.message, "No tasks running.");
    }
}
