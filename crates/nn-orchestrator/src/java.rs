//! Choosing a Java runtime (image tag) for a Minecraft version.
//!
//! | Minecraft          | Java | Tag      |
//! |--------------------|------|----------|
//! | 26.1 and later     | 25   | `java25` |
//! | 1.20.5 – 1.21.x    | 21   | `java21` |
//! | 1.17 – 1.20.4      | 17   | `java17` |
//! | before 1.17        | 8    | `java8`  |
//!
//! Forge before 1.18 must run on Java 8. `LATEST`/`SNAPSHOT` use the
//! image's `latest` tag, which tracks what the newest Minecraft requires.
//!
//! Sources: <https://docker-minecraft-server.readthedocs.io/en/latest/versions/java/>,
//! <https://minecraft.wiki/w/Java_Edition_26.1-snapshot-1> (Java 25 minimum).

/// The default image repository.
pub const DEFAULT_IMAGE: &str = "itzg/minecraft-server";

/// A parsed release version: legacy `1.minor.patch`, or year-based `26.1.x`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Version(u32, u32, u32);

fn parse(version: &str) -> Option<Version> {
    let mut parts = version.trim().split('.').map(|p| p.parse::<u32>().ok());
    let major = parts.next()??;
    let minor = parts.next().unwrap_or(Some(0))?;
    let patch = parts.next().unwrap_or(Some(0))?;
    Some(Version(major, minor, patch))
}

/// Whether the version follows the latest release automatically.
pub fn is_floating(mc_version: &str) -> bool {
    matches!(
        mc_version.trim().to_ascii_uppercase().as_str(),
        "LATEST" | "SNAPSHOT"
    )
}

/// The Java major version a server needs, or `None` for floating versions
/// (use the image's `latest` tag).
pub fn required_java(mc_version: &str, loader: &str) -> Option<u32> {
    if is_floating(mc_version) {
        return None;
    }
    let v = parse(mc_version)?;
    let java = if v.0 >= 26 {
        25
    } else if v >= Version(1, 20, 5) {
        21
    } else if v >= Version(1, 17, 0) {
        17
    } else {
        8
    };
    // Forge below 1.18 requires Java 8 (the image docs call this out).
    if loader.eq_ignore_ascii_case("forge") && v.0 == 1 && v.1 < 18 {
        return Some(8);
    }
    Some(java)
}

/// The image to use: `itzg/minecraft-server:<tag>` from the Java version
/// (an explicit override wins), falling back to `latest`.
pub fn default_image(mc_version: &str, loader: &str, java_override: Option<u32>) -> String {
    match java_override.or_else(|| required_java(mc_version, loader)) {
        Some(java) => format!("{DEFAULT_IMAGE}:java{java}"),
        None => format!("{DEFAULT_IMAGE}:latest"),
    }
}

/// Validates a Minecraft version string: `LATEST`, `SNAPSHOT`, a release
/// like `1.21.4` or `26.1`, or a snapshot/pre-release id like `24w14a` or
/// `1.21-pre1`.
pub fn is_valid_version(version: &str) -> bool {
    let v = version.trim();
    !v.is_empty()
        && v.len() <= 32
        && (is_floating(v)
            || v.bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_')))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_versions_to_java() {
        let cases = [
            ("26.1", "vanilla", Some(25)),
            ("26.1.2", "paper", Some(25)),
            ("1.21.4", "vanilla", Some(21)),
            ("1.20.5", "fabric", Some(21)),
            ("1.20.4", "paper", Some(17)),
            ("1.20.1", "forge", Some(17)),
            ("1.18.2", "forge", Some(17)),
            ("1.17.1", "vanilla", Some(17)),
            ("1.17.1", "forge", Some(8)),
            ("1.16.5", "forge", Some(8)),
            ("1.12.2", "vanilla", Some(8)),
            ("LATEST", "vanilla", None),
            ("snapshot", "vanilla", None),
            ("24w14a", "vanilla", None),
        ];
        for (version, loader, expected) in cases {
            assert_eq!(
                required_java(version, loader),
                expected,
                "{version} {loader}"
            );
        }
    }

    #[test]
    fn picks_images() {
        assert_eq!(
            default_image("1.21.4", "paper", None),
            "itzg/minecraft-server:java21"
        );
        assert_eq!(
            default_image("1.21.4", "paper", Some(25)),
            "itzg/minecraft-server:java25"
        );
        assert_eq!(
            default_image("LATEST", "vanilla", None),
            "itzg/minecraft-server:latest"
        );
    }

    #[test]
    fn validates_versions() {
        for ok in ["LATEST", "1.21.4", "26.1", "24w14a", "1.21-pre1"] {
            assert!(is_valid_version(ok), "{ok}");
        }
        for bad in ["", "1.21 4", "../../etc", "x".repeat(40).as_str()] {
            assert!(!is_valid_version(bad), "{bad}");
        }
    }
}
