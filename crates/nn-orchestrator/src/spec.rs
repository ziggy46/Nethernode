//! Translating a server's settings into a Docker container definition.
//!
//! Env vars follow the itzg/minecraft-server docs:
//! <https://docker-minecraft-server.readthedocs.io/en/latest/variables/> and
//! <https://docker-minecraft-server.readthedocs.io/en/latest/types-and-platforms/>.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use bollard::models::{
    ContainerCreateBody, HostConfig, PortBinding, RestartPolicy, RestartPolicyNameEnum,
};
use sha2::{Digest, Sha256};

/// Label marking containers NetherNode manages.
pub const LABEL_MANAGED: &str = "nethernode.managed";
/// Label holding the server ID.
pub const LABEL_SERVER_ID: &str = "nethernode.server_id";
/// Label holding a hash of the container definition, to detect drift.
pub const LABEL_SPEC_HASH: &str = "nethernode.spec_hash";

/// Port the game listens on inside the container.
pub const CONTAINER_GAME_PORT: &str = "25565/tcp";
/// RCON port inside the container (the image default).
pub const CONTAINER_RCON_PORT: &str = "25575/tcp";
/// Server management protocol port inside the container.
pub const CONTAINER_MGMT_PORT: &str = "25585/tcp";
/// [`CONTAINER_MGMT_PORT`] as a number, for `server.properties`.
pub const MGMT_PORT: u16 = 25585;

/// Env vars NetherNode manages; custom env can't override them.
pub const RESERVED_ENV: &[&str] = &[
    "EULA",
    "ENABLE_RCON",
    "RCON_PASSWORD",
    "RCON_PORT",
    "SERVER_PORT",
    "MEMORY",
    "TYPE",
    "VERSION",
    "UID",
    "GID",
    "MODPACK_PLATFORM",
    "MOD_PLATFORM",
    "MODRINTH_MODPACK",
    "MODRINTH_PROJECT",
    "MODRINTH_VERSION",
    "MODRINTH_VERSION_ID",
    "MODRINTH_LOADER",
    "CF_API_KEY",
    "CF_API_KEY_FILE",
    "CF_SLUG",
    "CF_PAGE_URL",
    "CF_FILE_ID",
    "CF_DOWNLOADS_REPO",
];

/// `server.properties` keys NetherNode controls, with why. The image
/// rewrites most of them from the container's environment on every start,
/// so editing them in the file doesn't stick (or, for the address and port,
/// would cut NetherNode off from the server).
pub const MANAGED_PROPERTIES: &[(&str, &str)] = &[
    (
        "motd",
        "Set under Server configuration above; NetherNode applies it on every start.",
    ),
    (
        "max-players",
        "Set under Server configuration above; NetherNode applies it on every start.",
    ),
    (
        "server-port",
        "NetherNode connects to the server on this port inside its container.",
    ),
    (
        "server-ip",
        "The server must listen on every address inside its container.",
    ),
    (
        "enable-rcon",
        "For sending console commands to the server over RCON.",
    ),
    (
        "rcon.port",
        "For sending console commands to the server over RCON.",
    ),
    ("rcon.password", "Generated and kept secret by NetherNode."),
    ("management-server-enabled", MGMT_REASON),
    ("management-server-host", MGMT_REASON),
    ("management-server-port", MGMT_REASON),
    ("management-server-secret", MGMT_REASON),
    ("management-server-tls-enabled", MGMT_REASON),
    ("management-server-tls-keystore", MGMT_REASON),
    ("management-server-tls-keystore-password", MGMT_REASON),
    ("management-server-allowed-origins", MGMT_REASON),
];

const MGMT_REASON: &str =
    "For managing players through the server's management protocol (Minecraft 1.21.9+).";

/// Container memory limit in MiB for a Java heap of `heap_mb`: the heap plus
/// `percent` of it, but at least `min_mb` more. Java needs a few hundred MB
/// beyond the heap (class metadata, compiled code, network buffers), more
/// with mods, and that doesn't shrink with small heaps: a 1 GB vanilla
/// 1.21 server peaks around 1.35 GiB while starting. The limit is a cap, not
/// a reservation, so headroom costs nothing unless it's used.
pub fn memory_limit_mb(heap_mb: i64, percent: u32, min_mb: u32) -> i64 {
    heap_mb + (heap_mb * i64::from(percent) / 100).max(i64::from(min_mb))
}

/// Extra memory for modded servers, in percent of the heap and at least in
/// MiB: a large modpack's class metadata, compiled code, and buffers need
/// 1.5–2 GB beyond the heap (a 242-mod Forge 1.20.1 pack with a 6 GB heap
/// runs at about 7.3 GB).
pub const MODDED_OVERHEAD: (u32, u32) = (40, 2048);

/// Whether a loader runs mods (and so needs more room beyond the heap).
pub fn is_modded(loader: &str) -> bool {
    matches!(
        loader,
        "fabric" | "quilt" | "forge" | "neoforge" | "modrinth" | "curseforge"
    )
}

/// Container memory limit for `loader`: modded servers get at least
/// [`MODDED_OVERHEAD`].
pub fn memory_limit_mb_for(loader: &str, heap_mb: i64, percent: u32, min_mb: u32) -> i64 {
    if is_modded(loader) {
        memory_limit_mb(
            heap_mb,
            percent.max(MODDED_OVERHEAD.0),
            min_mb.max(MODDED_OVERHEAD.1),
        )
    } else {
        memory_limit_mb(heap_mb, percent, min_mb)
    }
}

/// Supported server software: (loader id, `TYPE`, version-pin env var).
pub const LOADERS: &[(&str, &str, Option<&str>)] = &[
    ("vanilla", "VANILLA", None),
    ("paper", "PAPER", Some("PAPER_BUILD")),
    ("purpur", "PURPUR", Some("PURPUR_BUILD")),
    ("fabric", "FABRIC", Some("FABRIC_LOADER_VERSION")),
    ("forge", "FORGE", Some("FORGE_VERSION")),
    ("neoforge", "NEOFORGE", Some("NEOFORGE_VERSION")),
    ("quilt", "QUILT", Some("QUILT_LOADER_VERSION")),
    ("modrinth", "MODRINTH", None),
    ("curseforge", "AUTO_CURSEFORGE", None),
];

/// Loaders that install a modpack rather than plain server software.
pub const MODPACK_LOADERS: &[&str] = &["modrinth", "curseforge"];

/// Folder in a CurseForge modpack server's data where files the pack can't
/// download itself go (the image's downloads repo).
pub const CURSEFORGE_DOWNLOADS_DIR: &str = "downloads";

/// The modpack a server is installed from, stored as JSON with the server.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Modpack {
    /// `modrinth` or `curseforge`.
    pub platform: String,
    /// Platform project ID (Modrinth ID; CurseForge numeric ID, or empty
    /// when only the slug is known).
    pub project_id: String,
    /// URL name on the platform.
    pub slug: String,
    /// Display name.
    pub title: String,
    /// The pack version installed (Modrinth version ID, CurseForge file
    /// ID); `None` installs the newest.
    pub version_id: Option<String>,
    /// That version's name, for display.
    pub version_name: Option<String>,
    /// The pack's mod loader (`fabric`, `forge`, `neoforge`, `quilt`), if
    /// known.
    pub loader: Option<String>,
    /// Icon image URL.
    pub icon_url: Option<String>,
    /// The pack's page.
    pub page_url: Option<String>,
}

/// Everything needed to define a server's container.
#[derive(Debug, Clone)]
pub struct SpecInput<'a> {
    /// Server ID (label).
    pub server_id: &'a str,
    /// Slug (container name, data directory).
    pub slug: &'a str,
    /// Image including tag.
    pub image: &'a str,
    /// Loader id from [`LOADERS`].
    pub loader: &'a str,
    /// Loader/build pin.
    pub loader_version: Option<&'a str>,
    /// Minecraft version.
    pub mc_version: &'a str,
    /// Java heap in MiB.
    pub memory_mb: i64,
    /// Extra container memory over the heap, in percent.
    pub memory_overhead_percent: u32,
    /// Smallest extra container memory over the heap, in MiB.
    pub memory_overhead_min_mb: u32,
    /// CPU limit in cores.
    pub cpu_limit: Option<f64>,
    /// Host port for the game, bound to 127.0.0.1.
    pub game_port: u16,
    /// Host port for RCON, bound to 127.0.0.1.
    pub rcon_port: u16,
    /// Host port for the management protocol, bound to 127.0.0.1.
    pub mgmt_port: Option<u16>,
    /// RCON password (plaintext; passed to the container).
    pub rcon_password: &'a str,
    /// Whether the admin accepted the Minecraft EULA.
    pub eula_accepted: bool,
    /// Aikar's JVM flags.
    pub use_aikar_flags: bool,
    /// Server list message.
    pub motd: Option<&'a str>,
    /// Player limit.
    pub max_players: i64,
    /// Admin-supplied env (reserved names are ignored).
    pub extra_env: &'a BTreeMap<String, String>,
    /// Host directory holding all servers' data (absolute).
    pub servers_dir: &'a Path,
    /// Address game, RCON, and management ports are published on.
    pub publish_ip: std::net::IpAddr,
    /// User and group the server runs as, so its files stay manageable by
    /// NetherNode; `None` keeps the image default.
    pub run_as: Option<(u32, u32)>,
    /// The modpack, for modpack loaders.
    pub modpack: Option<&'a Modpack>,
    /// The admin's CurseForge API key; without one the image uses its own.
    pub curseforge_api_key: Option<&'a str>,
}

/// A container definition plus its name and drift hash.
#[derive(Debug, Clone)]
pub struct ContainerSpec {
    /// Container name.
    pub name: String,
    /// The create request.
    pub body: ContainerCreateBody,
    /// Hash of the definition (also stored as a label).
    pub hash: String,
}

/// The Docker `TYPE` for a loader id.
pub fn loader_type(loader: &str) -> Option<&'static str> {
    LOADERS
        .iter()
        .find(|(id, ..)| *id == loader)
        .map(|(_, ty, _)| *ty)
}

/// The container name for a slug.
pub fn container_name(slug: &str) -> String {
    format!("nethernode-{slug}")
}

/// Builds the container definition.
pub fn build(input: &SpecInput<'_>) -> ContainerSpec {
    let (_, ty, pin_var) = LOADERS
        .iter()
        .find(|(id, ..)| *id == input.loader)
        .copied()
        .unwrap_or(("vanilla", "VANILLA", None));

    // BTreeMap keeps env order stable, so the hash is too.
    let mut env: BTreeMap<String, String> = BTreeMap::new();
    for (key, value) in input.extra_env {
        if !RESERVED_ENV.contains(&key.as_str()) && is_env_name(key) {
            env.insert(key.clone(), value.clone());
        }
    }
    if input.eula_accepted {
        env.insert("EULA".into(), "TRUE".into());
    }
    env.insert("TYPE".into(), ty.into());
    // A CurseForge pack decides its own Minecraft version.
    if input.loader != "curseforge" {
        env.insert("VERSION".into(), input.mc_version.trim().to_owned());
    }
    if let Some(pack) = input.modpack {
        match input.loader {
            "modrinth" => {
                let project = if pack.project_id.is_empty() {
                    &pack.slug
                } else {
                    &pack.project_id
                };
                env.insert("MODRINTH_MODPACK".into(), project.clone());
                if let Some(version) = pack.version_id.as_ref().filter(|v| !v.is_empty()) {
                    env.insert("MODRINTH_VERSION".into(), version.clone());
                }
                if let Some(loader) = pack.loader.as_ref().filter(|l| !l.is_empty()) {
                    env.insert("MODRINTH_LOADER".into(), loader.clone());
                }
            }
            "curseforge" => {
                env.insert("CF_SLUG".into(), pack.slug.clone());
                if let Some(file) = pack.version_id.as_ref().filter(|v| !v.is_empty()) {
                    env.insert("CF_FILE_ID".into(), file.clone());
                }
                env.insert(
                    "CF_DOWNLOADS_REPO".into(),
                    format!("/data/{CURSEFORGE_DOWNLOADS_DIR}"),
                );
                if let Some(key) = input.curseforge_api_key.filter(|k| !k.is_empty()) {
                    env.insert("CF_API_KEY".into(), key.to_owned());
                }
            }
            _ => {}
        }
    }
    env.insert("MEMORY".into(), format!("{}M", input.memory_mb));
    env.insert("ENABLE_RCON".into(), "true".into());
    env.insert("SERVER_PORT".into(), "25565".into());
    env.insert("RCON_PASSWORD".into(), input.rcon_password.into());
    env.insert("USE_AIKAR_FLAGS".into(), input.use_aikar_flags.to_string());
    env.insert("MAX_PLAYERS".into(), input.max_players.to_string());
    if let Some((uid, gid)) = input.run_as {
        env.insert("UID".into(), uid.to_string());
        env.insert("GID".into(), gid.to_string());
    }
    if let Some(motd) = input.motd.filter(|m| !m.is_empty()) {
        env.insert("MOTD".into(), motd.into());
    }
    if let (Some(var), Some(pin)) = (pin_var, input.loader_version.filter(|v| !v.is_empty())) {
        env.insert(var.into(), pin.into());
    }

    let publish = |port: u16| {
        Some(vec![PortBinding {
            host_ip: Some(input.publish_ip.to_string()),
            host_port: Some(port.to_string()),
        }])
    };
    let mut port_bindings = HashMap::from([
        (CONTAINER_GAME_PORT.to_owned(), publish(input.game_port)),
        (CONTAINER_RCON_PORT.to_owned(), publish(input.rcon_port)),
    ]);
    let mut exposed = vec![
        CONTAINER_GAME_PORT.to_owned(),
        CONTAINER_RCON_PORT.to_owned(),
    ];
    if let Some(port) = input.mgmt_port {
        port_bindings.insert(CONTAINER_MGMT_PORT.to_owned(), publish(port));
        exposed.push(CONTAINER_MGMT_PORT.to_owned());
    }
    let memory_bytes = memory_limit_mb_for(
        input.loader,
        input.memory_mb,
        input.memory_overhead_percent,
        input.memory_overhead_min_mb,
    ) * 1024
        * 1024;
    let data_dir = input.servers_dir.join(input.slug);

    let host_config = HostConfig {
        binds: Some(vec![format!("{}:/data", data_dir.display())]),
        port_bindings: Some(port_bindings),
        memory: Some(memory_bytes),
        nano_cpus: input
            .cpu_limit
            .filter(|c| *c > 0.0)
            .map(|c| (c * 1e9) as i64),
        // NetherNode owns restarts (crash policy, sleep), so Docker never restarts.
        restart_policy: Some(RestartPolicy {
            name: Some(RestartPolicyNameEnum::NO),
            maximum_retry_count: None,
        }),
        ..Default::default()
    };
    let mut labels = HashMap::from([
        (LABEL_MANAGED.to_owned(), "true".to_owned()),
        (LABEL_SERVER_ID.to_owned(), input.server_id.to_owned()),
    ]);
    let mut body = ContainerCreateBody {
        image: Some(input.image.to_owned()),
        env: Some(env.iter().map(|(k, v)| format!("{k}={v}")).collect()),
        exposed_ports: Some(exposed),
        host_config: Some(host_config),
        // A TTY and open stdin let the console attach later.
        tty: Some(true),
        open_stdin: Some(true),
        labels: Some(labels.clone()),
        ..Default::default()
    };

    let hash = spec_hash(&body);
    labels.insert(LABEL_SPEC_HASH.to_owned(), hash.clone());
    body.labels = Some(labels);
    ContainerSpec {
        name: container_name(input.slug),
        body,
        hash,
    }
}

/// Stable hash of a definition (labels excluded so the hash label can live there).
fn spec_hash(body: &ContainerCreateBody) -> String {
    let mut normalized = body.clone();
    normalized.labels = None;
    // serde_json serializes HashMap keys in arbitrary order; go via Value,
    // whose maps are sorted.
    let value = serde_json::to_value(&normalized).unwrap_or_default();
    let digest = Sha256::digest(value.to_string().as_bytes());
    hex::encode(&digest[..12])
}

fn is_env_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && name
            .bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
        && !name.as_bytes()[0].is_ascii_digit()
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn input<'a>(extra: &'a BTreeMap<String, String>, dir: &'a Path) -> SpecInput<'a> {
        SpecInput {
            server_id: "01SERVER",
            slug: "smp",
            image: "itzg/minecraft-server:java21",
            loader: "forge",
            loader_version: Some("47.2.0"),
            mc_version: "1.20.1",
            memory_mb: 4096,
            memory_overhead_percent: 25,
            memory_overhead_min_mb: 1024,
            cpu_limit: Some(2.0),
            game_port: 30000,
            rcon_port: 30001,
            mgmt_port: None,
            rcon_password: "secret",
            eula_accepted: true,
            use_aikar_flags: true,
            motd: Some("Hello"),
            max_players: 20,
            extra_env: extra,
            servers_dir: dir,
            publish_ip: std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
            run_as: None,
            modpack: None,
            curseforge_api_key: None,
        }
    }

    fn env(spec: &ContainerSpec) -> Vec<String> {
        spec.body.env.clone().unwrap()
    }

    #[test]
    fn builds_the_container_definition() {
        let extra = BTreeMap::new();
        let dir = PathBuf::from("/srv/nethernode/servers");
        let spec = build(&input(&extra, &dir));
        assert_eq!(spec.name, "nethernode-smp");
        let env = env(&spec);
        for expected in [
            "EULA=TRUE",
            "TYPE=FORGE",
            "VERSION=1.20.1",
            "MEMORY=4096M",
            "FORGE_VERSION=47.2.0",
            "ENABLE_RCON=true",
            "RCON_PASSWORD=secret",
            "USE_AIKAR_FLAGS=true",
            "MOTD=Hello",
            "MAX_PLAYERS=20",
        ] {
            assert!(
                env.contains(&expected.to_owned()),
                "missing {expected}: {env:?}"
            );
        }
        let host = spec.body.host_config.as_ref().unwrap();
        assert_eq!(
            host.binds.as_deref(),
            Some(&["/srv/nethernode/servers/smp:/data".to_owned()][..])
        );
        assert_eq!(
            host.memory,
            Some((4096 + 2048) * 1024 * 1024),
            "a modded (Forge) server: heap + 2 GB"
        );
        assert_eq!(host.nano_cpus, Some(2_000_000_000));
        let game = &host.port_bindings.as_ref().unwrap()[CONTAINER_GAME_PORT]
            .as_ref()
            .unwrap()[0];
        assert_eq!(
            (game.host_ip.as_deref(), game.host_port.as_deref()),
            (Some("127.0.0.1"), Some("30000"))
        );
        assert_eq!(
            host.restart_policy.as_ref().unwrap().name,
            Some(RestartPolicyNameEnum::NO)
        );
        let labels = spec.body.labels.as_ref().unwrap();
        assert_eq!(labels[LABEL_SERVER_ID], "01SERVER");
        assert_eq!(labels[LABEL_SPEC_HASH], spec.hash);
    }

    #[test]
    fn memory_limit_has_a_floor() {
        assert_eq!(memory_limit_mb(1024, 25, 1024), 2048);
        assert_eq!(memory_limit_mb(2048, 25, 1024), 3072);
        assert_eq!(memory_limit_mb(4096, 25, 1024), 5120);
        assert_eq!(
            memory_limit_mb(8192, 25, 1024),
            10240,
            "big heaps keep the percentage"
        );
        assert_eq!(memory_limit_mb(1024, 25, 0), 1280);
    }

    #[test]
    fn modded_servers_get_more_room() {
        assert_eq!(memory_limit_mb_for("forge", 6144, 25, 1024), 6144 + 2457);
        assert_eq!(memory_limit_mb_for("modrinth", 4096, 25, 1024), 4096 + 2048);
        assert_eq!(memory_limit_mb_for("paper", 4096, 25, 1024), 5120);
        assert_eq!(memory_limit_mb_for("vanilla", 2048, 25, 1024), 3072);
    }

    #[test]
    fn management_port_is_localhost_only() {
        let extra = BTreeMap::new();
        let dir = PathBuf::from("/d");
        let mut i = input(&extra, &dir);
        assert!(
            !build(&i)
                .body
                .host_config
                .unwrap()
                .port_bindings
                .unwrap()
                .contains_key(CONTAINER_MGMT_PORT),
            "no binding until a port is allocated"
        );
        i.mgmt_port = Some(30002);
        let bindings = build(&i).body.host_config.unwrap().port_bindings.unwrap();
        let mgmt = &bindings[CONTAINER_MGMT_PORT].as_ref().unwrap()[0];
        assert_eq!(
            (mgmt.host_ip.as_deref(), mgmt.host_port.as_deref()),
            (Some("127.0.0.1"), Some("30002"))
        );
    }

    #[test]
    fn runs_as_the_given_user() {
        let extra = BTreeMap::from([("UID".to_owned(), "0".to_owned())]);
        let dir = PathBuf::from("/d");
        let mut i = input(&extra, &dir);
        assert!(
            !env(&build(&i)).iter().any(|e| e.starts_with("UID=")),
            "custom env can't pick the user"
        );
        i.run_as = Some((1001, 1002));
        let env = env(&build(&i));
        assert!(env.contains(&"UID=1001".to_owned()));
        assert!(env.contains(&"GID=1002".to_owned()));
    }

    #[test]
    fn eula_is_only_set_when_accepted() {
        let extra = BTreeMap::new();
        let dir = PathBuf::from("/d");
        let mut i = input(&extra, &dir);
        i.eula_accepted = false;
        assert!(!env(&build(&i)).iter().any(|e| e.starts_with("EULA=")));
    }

    #[test]
    fn custom_env_cannot_override_managed_values() {
        let extra = BTreeMap::from([
            ("RCON_PASSWORD".to_owned(), "leak".to_owned()),
            ("EULA".to_owned(), "TRUE".to_owned()),
            ("DIFFICULTY".to_owned(), "hard".to_owned()),
            ("bad name".to_owned(), "x".to_owned()),
        ]);
        let dir = PathBuf::from("/d");
        let mut i = input(&extra, &dir);
        i.eula_accepted = false;
        let env = env(&build(&i));
        assert!(env.contains(&"DIFFICULTY=hard".to_owned()));
        assert!(env.contains(&"RCON_PASSWORD=secret".to_owned()));
        assert!(!env.iter().any(|e| e.starts_with("EULA=")));
        assert!(!env.iter().any(|e| e.starts_with("bad name")));
    }

    #[test]
    fn hash_is_stable_and_tracks_changes() {
        let extra = BTreeMap::new();
        let dir = PathBuf::from("/d");
        let a = build(&input(&extra, &dir));
        assert_eq!(a.hash, build(&input(&extra, &dir)).hash, "deterministic");
        let mut i = input(&extra, &dir);
        i.memory_mb = 2048;
        assert_ne!(a.hash, build(&i).hash);
    }

    #[test]
    fn knows_every_loader() {
        for (id, ty, _) in LOADERS {
            assert_eq!(loader_type(id), Some(*ty));
        }
        assert_eq!(loader_type("bukkit"), None);
    }

    fn pack(platform: &str) -> Modpack {
        Modpack {
            platform: platform.into(),
            project_id: if platform == "modrinth" {
                "5FFgwNNP".into()
            } else {
                "285109".into()
            },
            slug: "some-pack".into(),
            title: "Some Pack".into(),
            version_id: Some("V1".into()),
            version_name: None,
            loader: Some("fabric".into()),
            icon_url: None,
            page_url: None,
        }
    }

    #[test]
    fn modrinth_packs_pin_project_and_version() {
        let extra = BTreeMap::from([("MODRINTH_VERSION".to_owned(), "hijack".to_owned())]);
        let dir = PathBuf::from("/srv/nn/servers");
        let pack = pack("modrinth");
        let spec = build(&SpecInput {
            loader: "modrinth",
            modpack: Some(&pack),
            ..input(&extra, &dir)
        });
        let env = env(&spec);
        for want in [
            "TYPE=MODRINTH",
            "MODRINTH_MODPACK=5FFgwNNP",
            "MODRINTH_VERSION=V1",
            "MODRINTH_LOADER=fabric",
            "VERSION=1.20.1",
        ] {
            assert!(env.contains(&want.to_owned()), "{want} in {env:?}");
        }
        assert!(
            !env.iter().any(|e| e == "MODRINTH_VERSION=hijack"),
            "custom env can't override"
        );
    }

    #[test]
    fn curseforge_packs_get_a_downloads_folder_and_the_key() {
        let extra = BTreeMap::new();
        let dir = PathBuf::from("/srv/nn/servers");
        let pack = pack("curseforge");
        let spec = build(&SpecInput {
            loader: "curseforge",
            modpack: Some(&pack),
            curseforge_api_key: Some("$2a$10$abc"),
            ..input(&extra, &dir)
        });
        let vars = env(&spec);
        for want in [
            "TYPE=AUTO_CURSEFORGE",
            "CF_SLUG=some-pack",
            "CF_FILE_ID=V1",
            "CF_DOWNLOADS_REPO=/data/downloads",
            "CF_API_KEY=$2a$10$abc",
        ] {
            assert!(vars.contains(&want.to_owned()), "{want} in {vars:?}");
        }
        assert!(
            !vars.iter().any(|e| e.starts_with("VERSION=")),
            "the pack picks its version"
        );
        let without_key = build(&SpecInput {
            loader: "curseforge",
            modpack: Some(&pack),
            ..input(&extra, &dir)
        });
        assert!(
            !env(&without_key)
                .iter()
                .any(|e| e.starts_with("CF_API_KEY"))
        );
    }
}
