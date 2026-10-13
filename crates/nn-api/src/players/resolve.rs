//! Turning a typed username (and maybe a known UUID) into the exact
//! identity a server matches on.
//!
//! Online-mode servers know players by their Mojang account. Offline-mode
//! servers derive a UUID from the exact username, so the server's own user
//! cache (names it has seen join) wins, then the offline formula. Names are
//! always validated, since they may reach an RCON command line.

use connectrpc::ConnectError;
use nn_players::profile::{Profile, normalize_uuid, offline_uuid, valid_name};

use super::backend::Ctx;

/// The identity to act on for `name` (and `uuid`, when the caller already
/// knows it from a list or a session).
pub(crate) async fn profile(
    ctx: &Ctx<'_>,
    name: &str,
    uuid: Option<&str>,
) -> Result<Profile, ConnectError> {
    let name = name.trim();
    if !valid_name(name) {
        return Err(ConnectError::invalid_argument(format!(
            "{name:?} isn't a valid Minecraft username (1–16 letters, digits, or underscores)."
        )));
    }
    if let Some(uuid) = uuid.and_then(normalize_uuid) {
        return Ok(Profile {
            uuid,
            name: name.to_owned(),
        });
    }
    if !ctx.online_mode {
        let cached = ctx
            .user_cache()
            .await
            .into_iter()
            .find(|e| e.name.eq_ignore_ascii_case(name));
        return Ok(match cached {
            Some(entry) => Profile {
                uuid: normalize_uuid(&entry.uuid).unwrap_or(entry.uuid),
                name: entry.name,
            },
            None => Profile {
                uuid: offline_uuid(name),
                name: name.to_owned(),
            },
        });
    }
    ctx.state
        .players
        .lookup(ctx.state.auth.store(), name)
        .await?
        .ok_or_else(|| ConnectError::not_found(format!("No Minecraft account is named {name}.")))
}
