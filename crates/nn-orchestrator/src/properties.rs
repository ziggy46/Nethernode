//! What the `server.properties` form knows about each key: label, group,
//! help text, and the values Minecraft accepts. Keys missing from a server's
//! file aren't shown (they differ between versions); keys in the file but
//! not here appear under "Other" as plain text.

use crate::spec::MANAGED_PROPERTIES;

/// Form section.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Group {
    /// Game rules most admins touch.
    General,
    /// World generation and size.
    World,
    /// Joining, permissions, and player limits.
    Players,
    /// Ports and connection behavior.
    Network,
    /// Everything else Minecraft defines.
    Advanced,
    /// Keys this table doesn't know.
    Other,
}

impl Group {
    /// Display order.
    pub const ALL: [Group; 6] = [
        Group::General,
        Group::World,
        Group::Players,
        Group::Network,
        Group::Advanced,
        Group::Other,
    ];

    /// Section title.
    pub fn label(self) -> &'static str {
        match self {
            Group::General => "General",
            Group::World => "World",
            Group::Players => "Players",
            Group::Network => "Network",
            Group::Advanced => "Advanced",
            Group::Other => "Other",
        }
    }
}

/// Accepted values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// `true` or `false`.
    Bool,
    /// A whole number in a range.
    Int {
        /// Smallest allowed.
        min: i64,
        /// Largest allowed.
        max: i64,
    },
    /// One of a fixed set.
    Choice(&'static [&'static str]),
    /// Free text.
    Text,
}

/// A known key.
#[derive(Debug, Clone, Copy)]
pub struct Field {
    /// The key in the file.
    pub key: &'static str,
    /// Form label.
    pub label: &'static str,
    /// Section.
    pub group: Group,
    /// One-sentence help.
    pub help: &'static str,
    /// Accepted values.
    pub kind: Kind,
}

const fn f(
    key: &'static str,
    label: &'static str,
    group: Group,
    kind: Kind,
    help: &'static str,
) -> Field {
    Field {
        key,
        label,
        group,
        help,
        kind,
    }
}

use Group::{Advanced, General, Network, Players, World};
use Kind::{Bool, Choice, Text};

const fn int(min: i64, max: i64) -> Kind {
    Kind::Int { min, max }
}

/// Every key the form understands, in display order within each group.
pub const FIELDS: &[Field] = &[
    f(
        "motd",
        "Message of the day",
        General,
        Text,
        "Shown under the server's name in the multiplayer list.",
    ),
    f(
        "difficulty",
        "Difficulty",
        General,
        Choice(&["peaceful", "easy", "normal", "hard"]),
        "How dangerous mobs are and whether hunger can kill. Paper and Purpur keep an existing world's difficulty; change it with /difficulty in the console.",
    ),
    f(
        "gamemode",
        "Default game mode",
        General,
        Choice(&["survival", "creative", "adventure", "spectator"]),
        "Game mode for new players.",
    ),
    f(
        "force-gamemode",
        "Force game mode",
        General,
        Bool,
        "Put players back in the default game mode every time they join.",
    ),
    f(
        "hardcore",
        "Hardcore",
        General,
        Bool,
        "Players are banned from the server when they die.",
    ),
    f(
        "pvp",
        "Player vs. player",
        General,
        Bool,
        "Let players damage each other.",
    ),
    f(
        "allow-flight",
        "Allow flight",
        General,
        Bool,
        "Don't kick players for flying (needed by some mods and plugins).",
    ),
    f(
        "enable-command-block",
        "Command blocks",
        General,
        Bool,
        "Let command blocks run commands.",
    ),
    f(
        "spawn-protection",
        "Spawn protection radius",
        General,
        int(0, 29_999_984),
        "Blocks around spawn that only operators can change. 0 turns it off.",
    ),
    f(
        "level-name",
        "World folder",
        World,
        Text,
        "Folder the world is loaded from and saved to.",
    ),
    f(
        "level-seed",
        "Seed",
        World,
        Text,
        "Seed for generating a new world. Has no effect on an existing one.",
    ),
    f(
        "level-type",
        "World type",
        World,
        Text,
        "Generator for new worlds, e.g. minecraft:normal, minecraft:flat, minecraft:amplified, minecraft:large_biomes.",
    ),
    f(
        "generator-settings",
        "Generator settings",
        World,
        Text,
        "JSON settings for the world type, e.g. the layers of a flat world.",
    ),
    f(
        "generate-structures",
        "Generate structures",
        World,
        Bool,
        "Villages, temples, and other structures in new chunks.",
    ),
    f(
        "allow-nether",
        "Allow the Nether",
        World,
        Bool,
        "Let players travel to the Nether.",
    ),
    f(
        "view-distance",
        "View distance",
        World,
        int(3, 32),
        "Chunks sent to each player, in each direction. Higher costs memory and bandwidth.",
    ),
    f(
        "simulation-distance",
        "Simulation distance",
        World,
        int(3, 32),
        "Chunks around each player where mobs move and crops grow.",
    ),
    f(
        "max-world-size",
        "World border radius",
        World,
        int(1, 29_999_984),
        "Largest distance from the center players can go.",
    ),
    f(
        "spawn-monsters",
        "Spawn monsters",
        World,
        Bool,
        "Hostile mobs spawn.",
    ),
    f(
        "spawn-animals",
        "Spawn animals",
        World,
        Bool,
        "Passive animals spawn.",
    ),
    f(
        "spawn-npcs",
        "Spawn villagers",
        World,
        Bool,
        "Villagers spawn.",
    ),
    f(
        "max-players",
        "Player limit",
        Players,
        int(0, 2_147_483_647),
        "Most players online at once.",
    ),
    f(
        "white-list",
        "Allowlist",
        Players,
        Bool,
        "Only players on the allowlist can join.",
    ),
    f(
        "enforce-whitelist",
        "Enforce allowlist",
        Players,
        Bool,
        "Kick players who aren't on the allowlist when it's reloaded.",
    ),
    f(
        "online-mode",
        "Verify accounts",
        Players,
        Bool,
        "Check every player with Mojang. Turn off only behind a proxy that verifies them.",
    ),
    f(
        "enforce-secure-profile",
        "Require signed chat",
        Players,
        Bool,
        "Only allow players whose chat messages are signed by Mojang.",
    ),
    f(
        "op-permission-level",
        "Operator permission level",
        Players,
        int(1, 4),
        "What operators may do, from 1 (bypass spawn protection) to 4 (everything).",
    ),
    f(
        "function-permission-level",
        "Function permission level",
        Players,
        int(1, 4),
        "Permission level of datapack functions.",
    ),
    f(
        "player-idle-timeout",
        "Idle kick (minutes)",
        Players,
        int(0, 2_147_483_647),
        "Kick players idle this long. 0 never kicks.",
    ),
    f(
        "hide-online-players",
        "Hide player list",
        Players,
        Bool,
        "Don't show who's online in the multiplayer list.",
    ),
    f(
        "entity-broadcast-range-percentage",
        "Entity visibility range (%)",
        Players,
        int(10, 1000),
        "How far away players can see entities, relative to the default.",
    ),
    f(
        "server-port",
        "Port",
        Network,
        int(1, 65_535),
        "Port the server listens on.",
    ),
    f(
        "server-ip",
        "Listen address",
        Network,
        Text,
        "Address the server listens on; empty means all.",
    ),
    f(
        "network-compression-threshold",
        "Compression threshold",
        Network,
        int(-1, 2_147_483_647),
        "Packets larger than this many bytes are compressed. -1 turns compression off.",
    ),
    f(
        "rate-limit",
        "Packet rate limit",
        Network,
        int(0, 2_147_483_647),
        "Kick players sending more packets per second than this. 0 turns it off.",
    ),
    f(
        "enable-status",
        "Show in server list",
        Network,
        Bool,
        "Answer server list pings. Off makes the server look offline.",
    ),
    f(
        "enable-query",
        "GameSpy query",
        Network,
        Bool,
        "Answer GameSpy4 query requests.",
    ),
    f(
        "query.port",
        "Query port",
        Network,
        int(1, 65_535),
        "Port for GameSpy4 queries.",
    ),
    f(
        "prevent-proxy-connections",
        "Block VPN connections",
        Network,
        Bool,
        "Kick players whose connection comes from a different address than Mojang saw.",
    ),
    f(
        "accepts-transfers",
        "Accept transfers",
        Network,
        Bool,
        "Let other servers send players here with /transfer.",
    ),
    f(
        "use-native-transport",
        "Native networking",
        Network,
        Bool,
        "Use Linux's faster networking where available.",
    ),
    f(
        "enable-rcon",
        "RCON",
        Network,
        Bool,
        "Remote console access.",
    ),
    f(
        "rcon.port",
        "RCON port",
        Network,
        int(1, 65_535),
        "Port for remote console access.",
    ),
    f(
        "rcon.password",
        "RCON password",
        Network,
        Text,
        "Password for remote console access.",
    ),
    f(
        "broadcast-rcon-to-ops",
        "Show RCON commands to operators",
        Advanced,
        Bool,
        "Tell online operators about commands sent over RCON.",
    ),
    f(
        "broadcast-console-to-ops",
        "Show console commands to operators",
        Advanced,
        Bool,
        "Tell online operators about commands sent from the console.",
    ),
    f(
        "log-ips",
        "Log IP addresses",
        Advanced,
        Bool,
        "Include player IP addresses in the server log.",
    ),
    f(
        "pause-when-empty-seconds",
        "Pause when empty (seconds)",
        Advanced,
        int(-1, 2_147_483_647),
        "Stop ticking the world after this long with nobody online. -1 never pauses.",
    ),
    f(
        "max-tick-time",
        "Watchdog timeout (ms)",
        Advanced,
        int(-1, 9_223_372_036_854_775_807),
        "Stop the server if one tick takes longer than this. -1 turns the watchdog off.",
    ),
    f(
        "max-chained-neighbor-updates",
        "Chained block update limit",
        Advanced,
        int(-1, 2_147_483_647),
        "Most consecutive block updates before skipping the rest.",
    ),
    f(
        "sync-chunk-writes",
        "Synchronous chunk saves",
        Advanced,
        Bool,
        "Safer world saves at some cost to performance.",
    ),
    f(
        "region-file-compression",
        "Region file compression",
        Advanced,
        Choice(&["deflate", "lz4", "none"]),
        "How chunk data is compressed on disk.",
    ),
    f(
        "enable-jmx-monitoring",
        "JMX monitoring",
        Advanced,
        Bool,
        "Expose tick timings over Java management extensions.",
    ),
    f(
        "require-resource-pack",
        "Require resource pack",
        Advanced,
        Bool,
        "Disconnect players who decline the resource pack.",
    ),
    f(
        "resource-pack",
        "Resource pack URL",
        Advanced,
        Text,
        "Direct download link to a resource pack zip.",
    ),
    f(
        "resource-pack-sha1",
        "Resource pack SHA-1",
        Advanced,
        Text,
        "Checksum of the resource pack, so clients can cache it.",
    ),
    f(
        "resource-pack-id",
        "Resource pack ID",
        Advanced,
        Text,
        "UUID identifying the resource pack.",
    ),
    f(
        "resource-pack-prompt",
        "Resource pack prompt",
        Advanced,
        Text,
        "Message shown when asking players to accept the pack.",
    ),
    f(
        "initial-enabled-packs",
        "Enabled data packs",
        Advanced,
        Text,
        "Data packs enabled when a world is created, comma-separated.",
    ),
    f(
        "initial-disabled-packs",
        "Disabled data packs",
        Advanced,
        Text,
        "Data packs not enabled automatically when a world is created.",
    ),
    f(
        "text-filtering-config",
        "Text filtering config",
        Advanced,
        Text,
        "Chat filtering service configuration.",
    ),
    f(
        "text-filtering-version",
        "Text filtering version",
        Advanced,
        int(0, 1),
        "Chat filtering service protocol version.",
    ),
    f(
        "bug-report-link",
        "Bug report link",
        Advanced,
        Text,
        "Link shown to players on the disconnect screen for reporting problems.",
    ),
    f(
        "management-server-enabled",
        "Management API",
        Network,
        Bool,
        "Minecraft's server management protocol.",
    ),
    f(
        "management-server-host",
        "Management API address",
        Network,
        Text,
        "Address the management API listens on.",
    ),
    f(
        "management-server-port",
        "Management API port",
        Network,
        int(0, 65_535),
        "Port for the management API.",
    ),
    f(
        "management-server-secret",
        "Management API secret",
        Network,
        Text,
        "Secret clients present to the management API.",
    ),
    f(
        "management-server-tls-enabled",
        "Management API TLS",
        Network,
        Bool,
        "Encrypt management API connections.",
    ),
    f(
        "management-server-tls-keystore",
        "Management API keystore",
        Network,
        Text,
        "PKCS12 keystore for management API TLS.",
    ),
    f(
        "management-server-tls-keystore-password",
        "Management API keystore password",
        Network,
        Text,
        "Password for the keystore.",
    ),
    f(
        "management-server-allowed-origins",
        "Management API origins",
        Network,
        Text,
        "Browser origins allowed to connect.",
    ),
    f(
        "debug",
        "Debug mode",
        Advanced,
        Bool,
        "Extra logging for diagnosing problems.",
    ),
];

/// The schema entry for `key`, if known.
pub fn field(key: &str) -> Option<&'static Field> {
    FIELDS.iter().find(|f| f.key == key)
}

/// Why `key` can't be edited here, if it can't.
pub fn managed_reason(key: &str) -> Option<&'static str> {
    MANAGED_PROPERTIES
        .iter()
        .find_map(|(k, reason)| (*k == key).then_some(*reason))
}

/// Whether `key`'s value must never be shown.
pub fn is_secret(key: &str) -> bool {
    matches!(
        key,
        "rcon.password" | "management-server-secret" | "management-server-tls-keystore-password"
    )
}

/// Checks a new value for `key`. Returns a message for the admin on failure.
pub fn validate(key: &str, value: &str) -> Result<(), String> {
    if key.is_empty() || key.chars().any(char::is_control) {
        return Err("that isn't a valid property name".into());
    }
    if let Some(reason) = managed_reason(key) {
        return Err(format!("{key} is managed by NetherNode. {reason}"));
    }
    let Some(field) = field(key) else {
        return Ok(());
    };
    match field.kind {
        Kind::Bool if value != "true" && value != "false" => {
            Err(format!("{} must be true or false", field.label))
        }
        Kind::Int { min, max } => match value.trim().parse::<i64>() {
            Ok(n) if (min..=max).contains(&n) => Ok(()),
            _ => Err(format!(
                "{} must be a whole number from {min} to {max}",
                field.label
            )),
        },
        Kind::Choice(options) if !options.contains(&value) => Err(format!(
            "{} must be one of: {}",
            field.label,
            options.join(", ")
        )),
        _ if value.contains(['\n', '\r']) => Err(format!("{} must be a single line", field.label)),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_unique() {
        for (i, a) in FIELDS.iter().enumerate() {
            assert!(
                FIELDS[i + 1..].iter().all(|b| b.key != a.key),
                "duplicate {}",
                a.key
            );
        }
    }

    #[test]
    fn managed_keys_are_known_and_rejected() {
        for (key, _) in MANAGED_PROPERTIES {
            assert!(field(key).is_some(), "{key} has no schema entry");
            assert!(validate(key, "x").is_err());
        }
    }

    #[test]
    fn validation() {
        assert!(validate("difficulty", "hard").is_ok());
        assert!(validate("difficulty", "nightmare").is_err());
        assert!(validate("pvp", "yes").is_err());
        assert!(validate("view-distance", "12").is_ok());
        assert!(validate("view-distance", "64").is_err());
        assert!(validate("view-distance", "ten").is_err());
        assert!(validate("level-seed", "a\nb").is_err());
        assert!(validate("some-mod-setting", "anything").is_ok());
        assert!(validate("", "x").is_err());
    }
}
