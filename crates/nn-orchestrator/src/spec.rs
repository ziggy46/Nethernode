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
];

/// Supported server software: (loader id, `TYPE`, version-pin env var).
pub const LOADERS: &[(&str, &str, Option<&str>)] = &[
    ("vanilla", "VANILLA", None),
    ("paper", "PAPER", Some("PAPER_BUILD")),
    ("purpur", "PURPUR", Some("PURPUR_BUILD")),
    ("fabric", "FABRIC", Some("FABRIC_LOADER_VERSION")),
    ("forge", "FORGE", Some("FORGE_VERSION")),
    ("neoforge", "NEOFORGE", Some("NEOFORGE_VERSION")),
    ("quilt", "QUILT", Some("QUILT_LOADER_VERSION")),
];

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
    /// CPU limit in cores.
    pub cpu_limit: Option<f64>,
    /// Host port for the game, bound to 127.0.0.1.
    pub game_port: u16,
    /// Host port for RCON, bound to 127.0.0.1.
    pub rcon_port: u16,
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
    env.insert("VERSION".into(), input.mc_version.trim().to_owned());
    env.insert("MEMORY".into(), format!("{}M", input.memory_mb));
    env.insert("ENABLE_RCON".into(), "true".into());
    env.insert("RCON_PASSWORD".into(), input.rcon_password.into());
    env.insert("USE_AIKAR_FLAGS".into(), input.use_aikar_flags.to_string());
    env.insert("MAX_PLAYERS".into(), input.max_players.to_string());
    if let Some(motd) = input.motd.filter(|m| !m.is_empty()) {
        env.insert("MOTD".into(), motd.into());
    }
    if let (Some(var), Some(pin)) = (pin_var, input.loader_version.filter(|v| !v.is_empty())) {
        env.insert(var.into(), pin.into());
    }

    let localhost = |port: u16| {
        Some(vec![PortBinding {
            host_ip: Some("127.0.0.1".into()),
            host_port: Some(port.to_string()),
        }])
    };
    let port_bindings = HashMap::from([
        (CONTAINER_GAME_PORT.to_owned(), localhost(input.game_port)),
        (CONTAINER_RCON_PORT.to_owned(), localhost(input.rcon_port)),
    ]);
    let memory_bytes =
        input.memory_mb * 1024 * 1024 * (100 + i64::from(input.memory_overhead_percent)) / 100;
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
        exposed_ports: Some(vec![CONTAINER_GAME_PORT.into(), CONTAINER_RCON_PORT.into()]),
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
            cpu_limit: Some(2.0),
            game_port: 30000,
            rcon_port: 30001,
            rcon_password: "secret",
            eula_accepted: true,
            use_aikar_flags: true,
            motd: Some("Hello"),
            max_players: 20,
            extra_env: extra,
            servers_dir: dir,
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
            Some(4096 * 1024 * 1024 * 125 / 100),
            "heap + 25%"
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
}
