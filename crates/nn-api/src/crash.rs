//! Why a server crashed or wouldn't start, read from its crash report or
//! log: the common causes NetherNode can name, and fix with a click where
//! it can (leave out the mods at fault, or use another Java).

use std::collections::{BTreeSet, HashMap};

use nn_files::{EntryKind, RelPath, TRASH_RETENTION};
use nn_node::Folder;
use nn_store::Server;
use tokio::io::AsyncReadExt;

use crate::AppState;
use crate::modpack_updates;

/// What went wrong, in terms NetherNode can act on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Cause {
    /// Mods that only run in the game itself (they load client code), by
    /// mod ID.
    ClientOnly(Vec<String>),
    /// Mods that need other mods that aren't there: (missing mod, needed
    /// by).
    MissingDependencies(Vec<(String, String)>),
    /// Mods that don't work with this Minecraft or loader version, by mod
    /// ID, with what they need.
    Incompatible(Vec<(String, String)>),
    /// A mod's mixins (its patches to Minecraft) failed, by mod ID.
    MixinFailed(Vec<String>),
    /// The server (or a mod) needs a newer Java.
    NeedsJava(u32),
    /// Java ran out of memory.
    OutOfMemory,
}

impl Cause {
    /// The mods at fault, for the ones a fix can leave out.
    pub(crate) fn culprits(&self) -> Vec<String> {
        match self {
            Self::ClientOnly(m) | Self::MixinFailed(m) => m.clone(),
            Self::Incompatible(m) => m.iter().map(|(id, _)| id.clone()).collect(),
            _ => Vec::new(),
        }
    }
}

/// Reads a crash report or log for the causes it shows, most actionable
/// first.
pub(crate) fn diagnose(text: &str) -> Vec<Cause> {
    let mut causes = Vec::new();
    let client = client_only(text);
    if !client.is_empty() {
        causes.push(Cause::ClientOnly(client));
    }
    let missing = missing_dependencies(text);
    if !missing.is_empty() {
        causes.push(Cause::MissingDependencies(missing));
    }
    let incompatible = incompatible(text);
    if !incompatible.is_empty() {
        causes.push(Cause::Incompatible(incompatible));
    }
    let mixins = mixin_failures(text);
    if !mixins.is_empty() {
        causes.push(Cause::MixinFailed(mixins));
    }
    if let Some(java) = needs_java(text) {
        causes.push(Cause::NeedsJava(java));
    }
    if text.contains("java.lang.OutOfMemoryError") {
        causes.push(Cause::OutOfMemory);
    }
    causes
}

/// Signs that code meant for the game's own window ran on a server.
fn loads_client_code(line: &str) -> bool {
    line.contains("for invalid dist DEDICATED_SERVER")
        || line.contains("com/mojang/blaze3d")
        || line.contains("com.mojang.blaze3d")
        || line.contains("net/minecraft/client/")
        || line.contains("net.minecraft.client.")
        || line.contains("in environment type SERVER")
}

fn client_only(text: &str) -> Vec<String> {
    let lines: Vec<&str> = text.lines().collect();
    let mut found = BTreeSet::new();
    for (i, line) in lines.iter().enumerate() {
        // Forge/NeoForge: "Failed to create mod instance. ModID: x, class …"
        // followed by the exception.
        if let Some(rest) = line.split("Failed to create mod instance. ModID: ").nth(1) {
            let id = rest.split([',', ' ']).next().unwrap_or_default();
            let window = &lines[i..lines.len().min(i + 4)];
            if !id.is_empty() && window.iter().any(|l| loads_client_code(l)) {
                found.insert(id.to_owned());
            }
        }
        // The crash report's per-mod sections: "-- MOD x --", then details.
        if let Some(id) = line
            .trim()
            .strip_prefix("-- MOD ")
            .and_then(|r| r.strip_suffix(" --"))
        {
            let window = &lines[i..lines.len().min(i + 8)];
            if window.iter().any(|l| loads_client_code(l)) {
                found.insert(id.to_owned());
            }
        }
        // Fabric/Quilt: "Could not execute entrypoint stage 'main' due to
        // errors, provided by 'x'" with a client class in the cause.
        if let Some(rest) = line.split("due to errors, provided by '").nth(1)
            && let Some(id) = rest.split('\'').next()
        {
            let window = &lines[i..lines.len().min(i + 12)];
            if window.iter().any(|l| loads_client_code(l)) {
                found.insert(id.to_owned());
            }
        }
    }
    found.into_iter().collect()
}

fn missing_dependencies(text: &str) -> Vec<(String, String)> {
    let mut found = BTreeSet::new();
    for line in text.lines() {
        // Forge/NeoForge: "Mod ID: 'x', Requested by: 'y', … '[MISSING]'"
        if line.contains("MISSING")
            && let (Some(id), Some(by)) = (
                between(line, "Mod ID: '", "'"),
                between(line, "Requested by: '", "'"),
            )
        {
            found.insert((id.to_owned(), by.to_owned()));
        }
        // Fabric: "Mod 'Y' (y) 1.0 requires any version of mod x, which is
        // missing!" (older: "requires mod x").
        if line.contains(", which is missing")
            && let Some(by) = between(line, "(", ")")
            && let Some(id) = line
                .split(" of mod ")
                .nth(1)
                .or_else(|| line.split("requires mod ").nth(1))
                .and_then(|r| r.split([',', ' ']).next())
        {
            found.insert((id.trim_matches('\'').to_owned(), by.to_owned()));
        }
    }
    found.into_iter().collect()
}

fn incompatible(text: &str) -> Vec<(String, String)> {
    let mut found = BTreeSet::new();
    for line in text.lines() {
        // Fabric: "Mod 'X' (x) 1.0 requires version 1.20.2 of minecraft,
        // but only the wrong version is present: 1.20.1!" (also for the
        // loader, fabric-api, java).
        if line.contains("but only the wrong version is present")
            && let Some(id) = between(line, "(", ")")
            && let Some(need) = line
                .split(" requires ")
                .nth(1)
                .and_then(|r| r.split(", but").next())
        {
            found.insert((id.to_owned(), need.trim().to_owned()));
        }
        // Forge/NeoForge: "Mod ID: 'minecraft', Requested by: 'x', Expected
        // range: '[1.20.2,)', Actual version: '1.20.1'"
        if line.contains("Actual version:")
            && !line.contains("MISSING")
            && let (Some(by), Some(range)) = (
                between(line, "Requested by: '", "'"),
                between(line, "Expected range: '", "'"),
            )
        {
            let what = between(line, "Mod ID: '", "'").unwrap_or("a dependency");
            found.insert((by.to_owned(), format!("{what} {range}")));
        }
    }
    found.into_iter().collect()
}

fn mixin_failures(text: &str) -> Vec<String> {
    let mut found = BTreeSet::new();
    for line in text.lines() {
        // "Mixin apply for mod x failed …" / "… from mod x failed injection check"
        for marker in ["Mixin apply for mod ", "from mod "] {
            if let Some(rest) = line.split(marker).nth(1)
                && (line.contains("failed") || line.contains("Failed"))
                && let Some(id) = rest.split([' ', ',', ':', ']']).next()
                && !id.is_empty()
            {
                found.insert(id.trim_matches('\'').to_owned());
            }
        }
    }
    found.into_iter().collect()
}

/// "UnsupportedClassVersionError: … class file version 65.0" means Java
/// 21 is needed (class file versions are Java's plus 44).
fn needs_java(text: &str) -> Option<u32> {
    text.lines()
        .filter(|l| l.contains("UnsupportedClassVersionError"))
        .filter_map(|l| {
            let version = l.split("class file version ").nth(1)?;
            let major: u32 = version.split('.').next()?.trim().parse().ok()?;
            major.checked_sub(44)
        })
        .max()
}

fn between<'a>(line: &'a str, start: &str, end: &str) -> Option<&'a str> {
    let rest = line.split(start).nth(1)?;
    rest.split(end).next().filter(|s| !s.is_empty())
}

/// How much of the end of the log is read when there's no crash report.
const LOG_TAIL: u64 = 1024 * 1024;
/// A crash report this much older than the last start belongs to an
/// earlier run.
const REPORT_SLACK_MS: i64 = 5 * 60 * 1000;

/// What the server's newest crash report (or its log) says, with the
/// file it was read from.
pub(crate) async fn read_crash(folder: &Folder, server: &Server) -> Option<(String, String)> {
    let since = server.last_started_at.unwrap_or(0) - REPORT_SLACK_MS;
    if let Ok(dir) = RelPath::parse("crash-reports")
        && let Ok(entries) = folder.list(&dir).await
        && let Some(newest) = entries
            .iter()
            .filter(|e| e.kind == EntryKind::File && e.name.ends_with(".txt"))
            .filter(|e| e.modified_ms >= since)
            .max_by_key(|e| e.modified_ms)
    {
        let path = format!("crash-reports/{}", newest.name);
        if let Some(text) = tail(folder, &path).await {
            return Some((path, text));
        }
    }
    let path = "logs/latest.log".to_owned();
    tail(folder, &path).await.map(|text| (path, text))
}

async fn tail(folder: &Folder, path: &str) -> Option<String> {
    let rel = RelPath::parse(path).ok()?;
    let reader = folder.open_read(&rel).await.ok()?;
    let start = reader.size().saturating_sub(LOG_TAIL);
    let mut stream = reader.from_offset(start).await.ok()?;
    let mut bytes = Vec::new();
    stream.read_to_end(&mut bytes).await.ok()?;
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

/// A jar in mods/ and what it says about itself.
#[derive(Debug, Clone)]
pub(crate) struct Jar {
    pub file: String,
    pub name: Option<String>,
}

/// The server's mods by mod ID.
pub(crate) async fn jars_by_id(folder: &Folder) -> HashMap<String, Jar> {
    let mut out = HashMap::new();
    let Ok(dir) = RelPath::parse("mods") else {
        return out;
    };
    let Ok(entries) = folder.list(&dir).await else {
        return out;
    };
    for entry in entries
        .iter()
        .filter(|e| e.kind == EntryKind::File && e.name.ends_with(".jar"))
    {
        let Ok(path) = RelPath::parse(&format!("mods/{}", entry.name)) else {
            continue;
        };
        if let Ok(Some(info)) = folder.mod_info(&path).await
            && let Some(id) = info.id.clone()
        {
            out.insert(
                id,
                Jar {
                    file: entry.name.clone(),
                    name: info.name.clone(),
                },
            );
        }
    }
    out
}

/// What a cause's fix does, or `None` when there's nothing to click.
pub(crate) fn fix_label(cause: &Cause, found: &HashMap<String, Jar>) -> Option<String> {
    match cause {
        Cause::NeedsJava(java) => Some(format!("Use Java {java} and start")),
        other => {
            let mods = other.culprits();
            let known = mods.iter().filter(|m| found.contains_key(*m)).count();
            (known > 0).then(|| {
                if known == 1 {
                    "Leave this mod out and start".to_owned()
                } else {
                    format!("Leave these {known} mods out and start")
                }
            })
        }
    }
}

/// Applies the fix for the cause of `kind` (see [`fix_label`]), then starts
/// the server. Returns what was done.
pub(crate) async fn fix(state: &AppState, server: &Server, kind: &str) -> Result<String, String> {
    let node = state.nodes.for_server(server);
    let folder = node.folder(server).await.map_err(|e| e.to_string())?;
    let (_, text) = read_crash(&folder, server)
        .await
        .ok_or("There's no crash report or log to read.")?;
    let cause = diagnose(&text)
        .into_iter()
        .find(|c| kind_of(c) == kind)
        .ok_or("That's no longer what the server's crash report says.")?;
    let done = match cause {
        Cause::NeedsJava(java) => {
            let mut settings = server.settings();
            settings.java_version = Some(i64::from(java));
            modpack_updates::apply(state, server, settings).await?;
            format!("{} now runs on Java {java}.", server.name)
        }
        other => leave_out(state, server, &folder, &other.culprits()).await?,
    };
    node.start(&server.id)
        .await
        .map_err(|e| format!("{done} It didn't start: {e}"))?;
    Ok(format!("{done} It's starting."))
}

/// Turns the mods off (`.jar.disabled`), and for packs the installer
/// would put back, has it leave them out from now on.
async fn leave_out(
    state: &AppState,
    server: &Server,
    folder: &Folder,
    mod_ids: &[String],
) -> Result<String, String> {
    let found = jars_by_id(folder).await;
    let jars: Vec<&Jar> = mod_ids.iter().filter_map(|id| found.get(id)).collect();
    if jars.is_empty() {
        return Err("None of those mods is in mods/ any more.".into());
    }
    // A pack installed from its modpack would download them again.
    let mut note = String::new();
    if let Some(mut pack) = modpack_updates::pack_of(server)
        && pack.server_pack_install().is_none()
    {
        let mut added = Vec::new();
        match pack.platform.as_str() {
            "curseforge" => {
                let key = state
                    .content
                    .curseforge_key()
                    .ok_or("Leaving mods out of a CurseForge pack needs the CurseForge API key (Settings).")?;
                let mut prints = HashMap::new();
                for jar in &jars {
                    let rel =
                        RelPath::parse(&format!("mods/{}", jar.file)).map_err(|e| e.to_string())?;
                    let reader = folder.open_read(&rel).await.map_err(|e| e.to_string())?;
                    let mut bytes = Vec::new();
                    reader
                        .from_offset(0)
                        .await
                        .map_err(|e| e.to_string())?
                        .read_to_end(&mut bytes)
                        .await
                        .map_err(|e| e.to_string())?;
                    prints.insert(nn_catalog::curseforge_fingerprint(&bytes), jar.file.clone());
                }
                let keys: Vec<u32> = prints.keys().copied().collect();
                let projects = state
                    .content
                    .catalog
                    .curseforge_identify(&key, &keys)
                    .await
                    .map_err(|e| {
                        format!("CurseForge couldn't be asked which mods these are: {e}")
                    })?;
                for (print, file) in &prints {
                    match projects.get(print) {
                        Some(project) => added.push(project.clone()),
                        None => {
                            note = format!(
                                " CurseForge didn't recognize {file}, so an update of the pack may bring it back."
                            )
                        }
                    }
                }
            }
            _ => added.extend(jars.iter().map(|j| j.file.clone())),
        }
        for item in added {
            if !pack.excluded.contains(&item) {
                pack.excluded.push(item);
            }
        }
        let mut settings = server.settings();
        settings.modpack_json = Some(serde_json::to_string(&pack).map_err(|e| e.to_string())?);
        modpack_updates::apply(state, server, settings).await?;
    }
    for jar in &jars {
        let from = RelPath::parse(&format!("mods/{}", jar.file)).map_err(|e| e.to_string())?;
        let to =
            RelPath::parse(&format!("mods/{}.disabled", jar.file)).map_err(|e| e.to_string())?;
        // An earlier attempt may have turned it off already.
        if folder.rename(&from, &to).await.is_err() {
            let _ = folder.delete(vec![from], TRASH_RETENTION).await;
        }
    }
    let names: Vec<String> = jars
        .iter()
        .map(|j| j.name.clone().unwrap_or_else(|| j.file.clone()))
        .collect();
    Ok(format!("Left out {}.{note}", names.join(", ")))
}

/// The name a cause goes by in the API.
pub(crate) fn kind_of(cause: &Cause) -> &'static str {
    match cause {
        Cause::ClientOnly(_) => "client_only",
        Cause::MissingDependencies(_) => "missing_dependencies",
        Cause::Incompatible(_) => "incompatible",
        Cause::MixinFailed(_) => "mixin_failed",
        Cause::NeedsJava(_) => "needs_java",
        Cause::OutOfMemory => "out_of_memory",
    }
}

/// A cause in a sentence or two.
pub(crate) fn summary(cause: &Cause) -> String {
    match cause {
        Cause::ClientOnly(m) => format!(
            "{} only run{} in the game itself, not on a server: {} load{} code for the game's window, which a server doesn't have.",
            plural(m.len(), "This mod", "These mods"),
            if m.len() == 1 { "s" } else { "" },
            if m.len() == 1 { "it" } else { "they" },
            if m.len() == 1 { "s" } else { "" },
        ),
        Cause::MissingDependencies(_) => {
            "Some mods need other mods that aren't installed. Add the missing ones (Mods › Add mods), or remove the mods that need them.".to_owned()
        }
        Cause::Incompatible(_) => {
            "Some mods are for another Minecraft or loader version than this server's.".to_owned()
        }
        Cause::MixinFailed(m) => format!(
            "{} couldn't patch Minecraft as {} expected, usually because of a version mismatch or another mod.",
            plural(m.len(), "A mod", "Some mods"),
            if m.len() == 1 { "it" } else { "they" },
        ),
        Cause::NeedsJava(java) => format!(
            "The server or one of its mods needs Java {java} or newer, but runs on an older Java."
        ),
        Cause::OutOfMemory => {
            "Java ran out of memory. Give the server more under Configuration › Memory allocation.".to_owned()
        }
    }
}

fn plural<'a>(n: usize, one: &'a str, many: &'a str) -> &'a str {
    if n == 1 { one } else { many }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_only_mods_in_a_forge_log() {
        // From Craft to Exile 2's first start.
        let log = "\
[17:48:53] [modloading-worker-0/ERROR] [ne.mi.fm.lo.RuntimeDistCleaner/DISTXFORM]: Attempted to load class net/minecraft/client/multiplayer/MultiPlayerGameMode for invalid dist DEDICATED_SERVER
[17:48:53] [modloading-worker-0/ERROR] [ne.mi.fm.ja.FMLModContainer/LOADING]: Failed to create mod instance. ModID: easier_sleeping, class de.geheimagentnr1.easier_sleeping.EasierSleeping
java.lang.RuntimeException: Attempted to load class net/minecraft/client/multiplayer/MultiPlayerGameMode for invalid dist DEDICATED_SERVER
[17:48:56] [modloading-worker-0/ERROR] [th.ko.LoggerKt/LOADING]: Failed to create mod instance. ModID: chatplus, class com.ebicep.chatplus.forge.ChatPlusForge
java.lang.NoClassDefFoundError: com/mojang/blaze3d/platform/InputConstants
-- MOD elytrautilities --
Details:
        Mod File: /data/mods/elytrautilities-forge-2.2.1+1.20.1.jar
        Failure message: Elytra Utilities (elytrautilities) has failed to load correctly
                java.lang.RuntimeException: Attempted to load class net/minecraft/client/Options for invalid dist DEDICATED_SERVER
-- MOD thinkbeforedrop --
Details:
        Failure message: Think Before Drop (thinkbeforedrop) has failed to load correctly
                java.lang.RuntimeException: Attempted to load class net/minecraft/client/gui/screens/Screen for invalid dist DEDICATED_SERVER
";
        assert_eq!(
            diagnose(log),
            [Cause::ClientOnly(vec![
                "chatplus".into(),
                "easier_sleeping".into(),
                "elytrautilities".into(),
                "thinkbeforedrop".into()
            ])]
        );
    }

    #[test]
    fn a_fabric_client_mod() {
        let log = "\
Caused by: net.fabricmc.loader.api.EntrypointException: Could not execute entrypoint stage 'main' due to errors, provided by 'zoomify' at 'dev.isxander.zoomify.Zoomify'!
Caused by: java.lang.RuntimeException: Cannot load class net.minecraft.class_310 in environment type SERVER
";
        assert_eq!(diagnose(log), [Cause::ClientOnly(vec!["zoomify".into()])]);
    }

    #[test]
    fn missing_and_wrong_versions() {
        let forge = "\
Missing or unsupported mandatory dependencies:
	Mod ID: 'geckolib', Requested by: 'mowziesmobs', Expected range: '[4.2,)', Actual version: '[MISSING]'
	Mod ID: 'minecraft', Requested by: 'newmod', Expected range: '[1.20.2,)', Actual version: '1.20.1'
";
        assert_eq!(
            diagnose(forge),
            [
                Cause::MissingDependencies(vec![("geckolib".into(), "mowziesmobs".into())]),
                Cause::Incompatible(vec![("newmod".into(), "minecraft [1.20.2,)".into())]),
            ]
        );
        let fabric = "\
 - Mod 'Sodium Extra' (sodium-extra) 0.5.1 requires any version of mod sodium, which is missing!
 - Mod 'Lithium' (lithium) 0.12 requires version 1.21 of minecraft, but only the wrong version is present: 1.20.1!
";
        assert_eq!(
            diagnose(fabric),
            [
                Cause::MissingDependencies(vec![("sodium".into(), "sodium-extra".into())]),
                Cause::Incompatible(vec![("lithium".into(), "version 1.21 of minecraft".into())]),
            ]
        );
    }

    #[test]
    fn java_and_memory() {
        let log = "Exception in thread \"main\" java.lang.UnsupportedClassVersionError: net/minecraft/server/Main has been compiled by a more recent version of the Java Runtime (class file version 65.0), this version of the Java Runtime only recognizes class file versions up to 61.0
java.lang.OutOfMemoryError: Java heap space";
        assert_eq!(diagnose(log), [Cause::NeedsJava(21), Cause::OutOfMemory]);
        assert!(diagnose("[Server thread/INFO]: Done (3.2s)!").is_empty());
    }

    #[test]
    fn mixin_failures_name_their_mod() {
        let log = "org.spongepowered.asm.mixin.transformer.throwables.MixinTransformerError: Mixin apply for mod brokenmod failed brokenmod.mixins.json:SomeMixin";
        assert_eq!(
            diagnose(log),
            [Cause::MixinFailed(vec!["brokenmod".into()])]
        );
    }
}
