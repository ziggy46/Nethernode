//! Player actions as console commands over RCON, for servers without the
//! management protocol (before 1.21.9, and modded servers), and for actions
//! the protocol doesn't cover (game mode, teleport, give).
//!
//! Commands are built only from validated parts: usernames must pass
//! [`valid_name`], so `@a` and other selectors can never be injected, and
//! free text (reasons) is flattened to one bounded line. Replies are
//! classified from the messages servers actually print; anything
//! unrecognized counts as success, since mods and old versions word things
//! differently and RCON itself reports no status.

use std::net::IpAddr;

use crate::profile::valid_name;

/// What a command did, judged from its reply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// It worked.
    Done(String),
    /// Nothing to do (already banned, already an operator, …).
    Unchanged(String),
    /// The command was refused; the text says why.
    Failed(String),
}

impl Outcome {
    /// Classifies a reply.
    pub fn from_reply(reply: &str) -> Self {
        let text = strip_formatting(reply).trim().to_owned();
        let lower = text.to_lowercase();
        const FAILED: &[&str] = &[
            "no player was found",
            "no entity was found",
            "unknown or incomplete command",
            "incorrect argument",
            "unknown item",
            "unknown command",
            "could not",
            "does not exist",
            "invalid ip",
            "invalid integer",
            "invalid float",
            "invalid uuid",
            "expected ",
            "you do not have permission",
        ];
        const UNCHANGED: &[&str] = &[
            "nothing changed",
            "already",
            "isn't banned",
            "is not banned",
        ];
        if FAILED.iter().any(|p| lower.contains(p)) {
            Outcome::Failed(text)
        } else if UNCHANGED.iter().any(|p| lower.contains(p)) {
            Outcome::Unchanged(text)
        } else {
            Outcome::Done(text)
        }
    }
}

/// Why a command couldn't be built.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CommandError {
    /// Not a valid username.
    #[error("{0:?} isn't a valid Minecraft username")]
    Name(String),
    /// Not a valid IP address.
    #[error("{0:?} isn't a valid IP address")]
    Ip(String),
    /// Not a valid item ID.
    #[error("{0:?} isn't a valid item ID")]
    Item(String),
    /// A number out of range.
    #[error("{0}")]
    Range(&'static str),
}

/// Game modes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GameMode {
    /// Survival.
    Survival,
    /// Creative.
    Creative,
    /// Adventure.
    Adventure,
    /// Spectator.
    Spectator,
}

impl GameMode {
    /// The command argument.
    pub fn as_str(self) -> &'static str {
        match self {
            GameMode::Survival => "survival",
            GameMode::Creative => "creative",
            GameMode::Adventure => "adventure",
            GameMode::Spectator => "spectator",
        }
    }
}

/// Where to teleport someone.
#[derive(Debug, Clone, PartialEq)]
pub enum Destination {
    /// To another player.
    Player(String),
    /// To coordinates.
    Coordinates(f64, f64, f64),
}

fn name(n: &str) -> Result<&str, CommandError> {
    if valid_name(n) {
        Ok(n)
    } else {
        Err(CommandError::Name(n.to_owned()))
    }
}

/// One line of at most 200 characters, without formatting codes.
fn reason(text: &str) -> String {
    let flat: String = strip_formatting(text)
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    flat.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(200)
        .collect()
}

fn with_reason(base: String, text: Option<&str>) -> String {
    match text.map(reason).filter(|r| !r.is_empty()) {
        Some(r) => format!("{base} {r}"),
        None => base,
    }
}

/// `kick <name> [reason]`.
pub fn kick(player: &str, message: Option<&str>) -> Result<String, CommandError> {
    Ok(with_reason(format!("kick {}", name(player)?), message))
}

/// `ban <name> [reason]`.
pub fn ban(player: &str, why: Option<&str>) -> Result<String, CommandError> {
    Ok(with_reason(format!("ban {}", name(player)?), why))
}

/// `pardon <name>`.
pub fn pardon(player: &str) -> Result<String, CommandError> {
    Ok(format!("pardon {}", name(player)?))
}

/// `ban-ip <ip> [reason]`.
pub fn ban_ip(ip: &str, why: Option<&str>) -> Result<String, CommandError> {
    let ip: IpAddr = ip.parse().map_err(|_| CommandError::Ip(ip.to_owned()))?;
    Ok(with_reason(format!("ban-ip {ip}"), why))
}

/// `pardon-ip <ip>`.
pub fn pardon_ip(ip: &str) -> Result<String, CommandError> {
    let ip: IpAddr = ip.parse().map_err(|_| CommandError::Ip(ip.to_owned()))?;
    Ok(format!("pardon-ip {ip}"))
}

/// `op <name>` / `deop <name>`.
pub fn op(player: &str, grant: bool) -> Result<String, CommandError> {
    Ok(format!(
        "{} {}",
        if grant { "op" } else { "deop" },
        name(player)?
    ))
}

/// `whitelist add|remove <name>`.
pub fn whitelist(player: &str, add: bool) -> Result<String, CommandError> {
    Ok(format!(
        "whitelist {} {}",
        if add { "add" } else { "remove" },
        name(player)?
    ))
}

/// `whitelist on|off`.
pub fn whitelist_enabled(on: bool) -> String {
    format!("whitelist {}", if on { "on" } else { "off" })
}

/// `gamemode <mode> <name>`.
pub fn gamemode(player: &str, mode: GameMode) -> Result<String, CommandError> {
    Ok(format!("gamemode {} {}", mode.as_str(), name(player)?))
}

/// `tp <name> <target>` or `tp <name> <x> <y> <z>`.
pub fn teleport(player: &str, to: &Destination) -> Result<String, CommandError> {
    let who = name(player)?;
    match to {
        Destination::Player(target) => Ok(format!("tp {who} {}", name(target)?)),
        Destination::Coordinates(x, y, z) => {
            const LIMIT: f64 = 30_000_000.0;
            if [x, y, z].iter().any(|v| !v.is_finite() || v.abs() > LIMIT) {
                return Err(CommandError::Range(
                    "coordinates must be numbers within the world border",
                ));
            }
            Ok(format!("tp {who} {x} {y} {z}"))
        }
    }
}

/// `give <name> <item> <count>`.
pub fn give(player: &str, item: &str, count: u32) -> Result<String, CommandError> {
    let who = name(player)?;
    let valid = !item.is_empty()
        && item.len() <= 100
        && item
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"_.:/-".contains(&b));
    if !valid {
        return Err(CommandError::Item(item.to_owned()));
    }
    if !(1..=6400).contains(&count) {
        return Err(CommandError::Range("give between 1 and 6400 items"));
    }
    Ok(format!("give {who} {item} {count}"))
}

/// Names from a `list` reply. Formats vary: `There are 2 of a max of 20
/// players online: a, b` (modern), `There are 2/20 players online:\na, b`
/// (old), plugins prefix names with ranks or tags.
pub fn parse_list(reply: &str) -> Vec<String> {
    let text = strip_formatting(reply);
    let Some((_, names)) = text.split_once(':') else {
        return Vec::new();
    };
    names
        .split([',', '\n', ' '])
        .map(|token| {
            token
                .trim()
                .trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '_')
        })
        .filter(|token| valid_name(token))
        .map(str::to_owned)
        .collect()
}

/// Removes `§` formatting codes and ANSI escapes.
pub fn strip_formatting(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '§' => {
                chars.next();
            }
            '\x1b' => {
                if chars.peek() == Some(&'[') {
                    chars.next();
                    for c in chars.by_ref() {
                        if c.is_ascii_alphabetic() {
                            break;
                        }
                    }
                }
            }
            _ => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replies_from_a_real_server() {
        // Captured from vanilla 26.3 over RCON.
        let done = [
            "Banned Notch: Being Notch",
            "Unbanned Notch",
            "Banned IP 203.0.113.9: spam",
            "Unbanned IP 203.0.113.9",
            "Made jeb_ a server operator",
            "Made jeb_ no longer a server operator",
            "Added jeb_ to the whitelist",
            "Removed jeb_ from the whitelist",
            "Whitelist is now turned off",
        ];
        for reply in done
            .into_iter()
            .chain(["Made InvalidJoe a server operator"])
        {
            assert!(
                matches!(Outcome::from_reply(reply), Outcome::Done(_)),
                "{reply}"
            );
        }
        let unchanged = [
            "Nothing changed. The player is already banned",
            "Nothing changed. The player isn't banned",
            "Nothing changed. The player is already an operator",
            "Nothing changed. The player is not an operator",
            "Player is already whitelisted",
            "Whitelist is already turned on",
        ];
        for reply in unchanged {
            assert!(
                matches!(Outcome::from_reply(reply), Outcome::Unchanged(_)),
                "{reply}"
            );
        }
        for reply in [
            "No player was found",
            "No entity was found",
            "Unknown or incomplete command, see below for error",
        ] {
            assert!(
                matches!(Outcome::from_reply(reply), Outcome::Failed(_)),
                "{reply}"
            );
        }
    }

    #[test]
    fn commands_refuse_injection() {
        assert_eq!(
            ban("Notch", Some("griefing\nspawn §cnow")).unwrap(),
            "ban Notch griefing spawn now"
        );
        assert!(ban("@a", None).is_err());
        assert!(op("jeb_ @a", true).is_err());
        assert!(kick("ThisNameIsWayTooLongForMC", None).is_err());
        assert!(ban_ip("1.2.3.4; op @a", None).is_err());
        assert_eq!(ban_ip("203.0.113.9", None).unwrap(), "ban-ip 203.0.113.9");
        assert!(give("Steve", "minecraft:diamond 64 @a", 1).is_err());
        assert!(give("Steve", "minecraft:diamond", 0).is_err());
        assert_eq!(
            give("Steve", "minecraft:diamond", 2).unwrap(),
            "give Steve minecraft:diamond 2"
        );
        assert_eq!(
            teleport("Steve", &Destination::Coordinates(1.5, 64.0, -3.0)).unwrap(),
            "tp Steve 1.5 64 -3"
        );
        assert!(teleport("Steve", &Destination::Coordinates(f64::NAN, 0.0, 0.0)).is_err());
        assert_eq!(
            gamemode("Steve", GameMode::Creative).unwrap(),
            "gamemode creative Steve"
        );
        assert_eq!(whitelist("jeb_", true).unwrap(), "whitelist add jeb_");
    }

    #[test]
    fn list_formats() {
        assert_eq!(
            parse_list("There are 0 of a max of 20 players online: "),
            Vec::<String>::new()
        );
        assert_eq!(
            parse_list("There are 2 of a max of 20 players online: jeb_, Notch"),
            vec!["jeb_", "Notch"]
        );
        assert_eq!(
            parse_list("There are 2/20 players online:\njeb_, Notch"),
            vec!["jeb_", "Notch"]
        );
        assert_eq!(
            parse_list(
                "§6There are §c1§6 out of maximum §c20§6 players online.\n§6default§r: §fSteve"
            ),
            vec!["Steve"],
            "group labels before the names are skipped"
        );
        assert_eq!(parse_list("no colon here"), Vec::<String>::new());
    }
}
