//! Reaching one server for player actions, the best available way:
//!
//! - **Management protocol** (1.21.9+) while running: lists, bans,
//!   operators, allowlist, and kicks, with change notifications.
//! - **RCON** while running, for older and modded servers, and for game
//!   mode, teleport, and give on every version.
//! - **Files** while stopped: Minecraft's list files, which the server
//!   loads when it starts. Only once Docker confirms nothing is running.

use std::sync::Arc;

use connectrpc::ConnectError;
use nn_files::{EDITOR_LIMIT, Expect, Properties, RelPath};
use nn_node::{Folder, Node};
use nn_players::commands::{self, Destination, GameMode, Outcome};
use nn_players::lists::{self, BanEntry, ListEntry, OpEntry};
use nn_players::mgmt::{MgmtClient, MgmtError, Operator, UserBan};
use nn_players::profile::{Profile, normalize_uuid};
use nn_store::Server;
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::AppState;
use crate::files::file_error;
use crate::proto::nethernode::v1 as pb;
use crate::rpc::servers::node_error;
use crate::rpc::store_error;

/// Source recorded on bans NetherNode places.
pub(crate) const SOURCE: &str = "NetherNode";

/// How a server is reached right now.
pub(crate) enum Via {
    Mgmt(Arc<MgmtClient>),
    Rcon,
    Files,
    Unavailable(String),
}

/// A server opened for player actions.
pub(crate) struct Ctx<'a> {
    pub state: &'a AppState,
    pub server: Server,
    pub via: Via,
    pub online_mode: bool,
    node: Node,
}

/// Minecraft's lists for one server, from whichever source applies.
#[derive(Default)]
pub(crate) struct Lists {
    pub ops: Vec<(Profile, u8)>,
    pub allowlist: Vec<Profile>,
    /// `(player, reason, expires unix ms, source)`.
    pub bans: Vec<(Profile, Option<String>, Option<i64>, String)>,
    pub allowlist_enabled: bool,
}

fn profile(uuid: &str, name: &str) -> Profile {
    Profile {
        uuid: normalize_uuid(uuid).unwrap_or_else(|| uuid.to_owned()),
        name: name.to_owned(),
    }
}

fn mgmt_error(err: MgmtError) -> ConnectError {
    match err {
        MgmtError::Rpc { message, .. } => ConnectError::failed_precondition(message),
        other => ConnectError::unavailable(other.to_string()),
    }
}

impl<'a> Ctx<'a> {
    /// Opens `server_id`, choosing how to reach it.
    pub async fn open(state: &'a AppState, server_id: &str) -> Result<Ctx<'a>, ConnectError> {
        let server = state
            .auth
            .store()
            .find_server(server_id)
            .await
            .map_err(store_error)?
            .ok_or_else(|| ConnectError::not_found("server not found"))?;
        let node = state.nodes.for_server(&server).clone();
        let available = node.unavailable().is_none();
        let via = match (server.status.as_str(), available) {
            (_, false) => Via::Unavailable(
                "Docker isn't available, so NetherNode can't tell whether the server is running."
                    .into(),
            ),
            ("running", true) => match state.players.mgmt(&node, &server).await {
                Some(client) => Via::Mgmt(client),
                None => Via::Rcon,
            },
            ("stopped" | "sleeping" | "crashed", true) => {
                if node
                    .container_running(&server.id)
                    .await
                    .map_err(node_error)?
                {
                    Via::Unavailable(
                        "The server is still shutting down. Try again in a moment.".into(),
                    )
                } else {
                    Via::Files
                }
            }
            ("installing", _) => Via::Unavailable("The server is still installing.".into()),
            ("starting", _) => {
                Via::Unavailable("The server is starting. Try again once it's running.".into())
            }
            _ => Via::Unavailable("The server is stopping. Try again once it has stopped.".into()),
        };
        let mut ctx = Ctx {
            state,
            server,
            via,
            online_mode: true,
            node,
        };
        ctx.online_mode = ctx.read_online_mode().await;
        Ok(ctx)
    }

    /// Opens a stopped server for file edits regardless of how it would
    /// normally be reached (before-start hooks, where it isn't running yet).
    pub async fn open_stopped(state: &'a AppState, server: Server) -> Ctx<'a> {
        let node = state.nodes.for_server(&server).clone();
        let mut ctx = Ctx {
            state,
            server,
            via: Via::Files,
            online_mode: true,
            node,
        };
        ctx.online_mode = ctx.read_online_mode().await;
        ctx
    }

    /// What can be done now.
    pub fn capabilities(&self) -> pb::Capabilities {
        let (backend, running, reason) = match &self.via {
            Via::Mgmt(_) => ("management", true, String::new()),
            Via::Rcon => ("rcon", true, String::new()),
            Via::Files => ("files", false, String::new()),
            Via::Unavailable(why) => ("unavailable", false, why.clone()),
        };
        let lists = !matches!(self.via, Via::Unavailable(_));
        pb::Capabilities {
            backend: backend.into(),
            unavailable_reason: reason,
            kick: running,
            ban: lists,
            op: lists,
            op_levels: matches!(self.via, Via::Mgmt(_) | Via::Files),
            allowlist: lists,
            game_mode: running,
            teleport: running,
            give: running,
            online_mode: self.online_mode,
            ..Default::default()
        }
    }

    fn unavailable(&self) -> ConnectError {
        match &self.via {
            Via::Unavailable(why) => ConnectError::failed_precondition(why.clone()),
            _ => ConnectError::failed_precondition("That isn't possible right now."),
        }
    }

    // ----- reading -----

    async fn files(&self) -> Result<Folder, ConnectError> {
        crate::files::open(self.state, &self.server.id)
            .await
            .map(|(_, f)| f)
    }

    async fn read_file(&self, path: &str) -> Result<Vec<u8>, ConnectError> {
        let files = match self.files().await {
            Ok(files) => files,
            // No folder yet (never started): nothing to read.
            Err(_) => return Ok(Vec::new()),
        };
        let path = RelPath::parse(path).map_err(file_error)?;
        let content = files
            .read_if_exists(&path, EDITOR_LIMIT)
            .await
            .map_err(file_error)?;
        Ok(content.map(|c| c.data).unwrap_or_default())
    }

    async fn properties(&self) -> Properties {
        let bytes = self
            .read_file("server.properties")
            .await
            .unwrap_or_default();
        Properties::parse(&String::from_utf8_lossy(&bytes))
    }

    async fn read_online_mode(&self) -> bool {
        let env: std::collections::BTreeMap<String, String> =
            serde_json::from_str(&self.server.env_json).unwrap_or_default();
        if let Some(value) = env.get("ONLINE_MODE") {
            return !value.eq_ignore_ascii_case("false");
        }
        self.properties().await.get("online-mode") != Some("false")
    }

    /// The server's own user cache: names and UUIDs it has seen.
    pub async fn user_cache(&self) -> Vec<lists::CacheEntry> {
        lists::parse(&self.read_file(lists::USER_CACHE).await.unwrap_or_default())
    }

    /// Operators, allowlist, and bans.
    pub async fn lists(&self) -> Result<Lists, ConnectError> {
        if let Via::Mgmt(client) = &self.via {
            let ops = client.operators().await.map_err(mgmt_error)?;
            let allowlist = client.allowlist().await.map_err(mgmt_error)?;
            let bans = client.bans().await.map_err(mgmt_error)?;
            let enabled = client.allowlist_enabled().await.map_err(mgmt_error)?;
            return Ok(Lists {
                ops: ops
                    .into_iter()
                    .map(|o| {
                        (
                            profile(&o.player.uuid, &o.player.name),
                            o.permission_level.unwrap_or(4),
                        )
                    })
                    .collect(),
                allowlist: allowlist
                    .into_iter()
                    .map(|p| profile(&p.uuid, &p.name))
                    .collect(),
                bans: bans
                    .into_iter()
                    .map(|b| {
                        let expires = b
                            .expires
                            .as_deref()
                            .and_then(|e| e.parse::<jiff::Timestamp>().ok())
                            .map(|t| t.as_millisecond());
                        (
                            profile(&b.player.uuid, &b.player.name),
                            b.reason,
                            expires,
                            b.source.unwrap_or_default(),
                        )
                    })
                    .collect(),
                allowlist_enabled: enabled,
            });
        }
        let ops: Vec<OpEntry> = lists::parse(&self.read_file(lists::OPS).await?);
        let allow: Vec<ListEntry> = lists::parse(&self.read_file(lists::WHITELIST).await?);
        let bans: Vec<BanEntry> = lists::parse(&self.read_file(lists::BANNED_PLAYERS).await?);
        Ok(Lists {
            ops: ops
                .into_iter()
                .map(|o| (profile(&o.uuid, &o.name), o.level))
                .collect(),
            allowlist: allow
                .into_iter()
                .map(|e| profile(&e.uuid, &e.name))
                .collect(),
            bans: bans
                .into_iter()
                .map(|b| {
                    let expires = lists::parse_mc_date(&b.expires);
                    (profile(&b.uuid, &b.name), b.reason, expires, b.source)
                })
                .collect(),
            allowlist_enabled: self.properties().await.get("white-list") == Some("true"),
        })
    }

    // ----- acting -----

    async fn rcon(
        &self,
        command: Result<String, commands::CommandError>,
    ) -> Result<Outcome, ConnectError> {
        let command = command.map_err(|e| ConnectError::invalid_argument(e.to_string()))?;
        if self.node.unavailable().is_some() {
            return Err(self.unavailable());
        }
        let reply = self
            .node
            .command(&self.server.id, &command)
            .await
            .map_err(node_error)?;
        match Outcome::from_reply(&reply) {
            Outcome::Failed(text) => Err(ConnectError::failed_precondition(text)),
            outcome => Ok(outcome),
        }
    }

    /// Edits a list file while the server is stopped.
    async fn edit_list<T, F>(&self, file: &'static str, edit: F) -> Result<(), ConnectError>
    where
        T: Serialize + DeserializeOwned + Send + 'static,
        F: FnOnce(&mut Vec<T>) + Send + 'static,
    {
        let files = self.files().await?;
        let path = RelPath::parse(file).map_err(file_error)?;
        let (bytes, expect) = match files
            .read_if_exists(&path, EDITOR_LIMIT)
            .await
            .map_err(file_error)?
        {
            Some(content) => (content.data, Expect::Version(content.version)),
            None => (Vec::new(), Expect::New),
        };
        let mut list: Vec<T> = lists::parse(&bytes);
        edit(&mut list);
        files
            .write(&path, lists::to_bytes(&list), expect)
            .await
            .map(drop)
            .map_err(file_error)
    }

    async fn set_property(
        &self,
        key: &'static str,
        value: &'static str,
    ) -> Result<(), ConnectError> {
        let files = self.files().await?;
        let path = RelPath::parse("server.properties").map_err(file_error)?;
        let (mut props, expect) = match files
            .read_if_exists(&path, EDITOR_LIMIT)
            .await
            .map_err(file_error)?
        {
            Some(content) => (
                Properties::parse(&String::from_utf8_lossy(&content.data)),
                Expect::Version(content.version),
            ),
            None => (Properties::default(), Expect::New),
        };
        props.set(key, value);
        files
            .write(&path, props.to_string().into_bytes(), expect)
            .await
            .map(drop)
            .map_err(file_error)
    }

    /// The management protocol drops entries it can't verify (an
    /// offline-mode player it has never seen); say so instead of
    /// pretending it worked.
    fn check_kept(&self, kept: bool, player: &Profile) -> Result<(), ConnectError> {
        if kept {
            return Ok(());
        }
        Err(ConnectError::failed_precondition(if self.online_mode {
            format!("The server didn't accept {}.", player.name)
        } else {
            format!(
                "This server runs in offline mode and hasn't seen {} yet, so it can't match them. Ask them to join once, or make the change while the server is stopped.",
                player.name
            )
        }))
    }

    pub async fn kick(&self, player: &Profile, message: &str) -> Result<(), ConnectError> {
        match &self.via {
            Via::Mgmt(client) => {
                let kicked = client.kick(player, message).await.map_err(mgmt_error)?;
                if kicked.is_empty() {
                    return Err(ConnectError::failed_precondition(format!(
                        "{} isn't online.",
                        player.name
                    )));
                }
                Ok(())
            }
            Via::Rcon => self
                .rcon(commands::kick(
                    &player.name,
                    Some(message).filter(|m| !m.is_empty()),
                ))
                .await
                .map(drop),
            _ => {
                Err(self.unavailable_or("Players can only be kicked while the server is running."))
            }
        }
    }

    fn unavailable_or(&self, message: &str) -> ConnectError {
        match &self.via {
            Via::Unavailable(why) => ConnectError::failed_precondition(why.clone()),
            _ => ConnectError::failed_precondition(message.to_owned()),
        }
    }

    pub async fn ban(
        &self,
        player: &Profile,
        reason: Option<&str>,
        expires_ms: Option<i64>,
    ) -> Result<(), ConnectError> {
        match &self.via {
            Via::Mgmt(client) => {
                let ban = UserBan {
                    player: player.clone(),
                    reason: reason.map(str::to_owned),
                    source: Some(SOURCE.into()),
                    expires: expires_ms
                        .and_then(|ms| jiff::Timestamp::from_millisecond(ms).ok())
                        .map(|t| t.to_string()),
                };
                let list = client.ban(&ban).await.map_err(mgmt_error)?;
                let kept = list.iter().any(|b| b.player.uuid == player.uuid);
                // An offline-mode server drops players it has never seen;
                // the ban still holds, as the proxy refuses them by name.
                if !kept && !self.online_mode {
                    return Ok(());
                }
                self.check_kept(kept, player)
            }
            // RCON bans are permanent; NetherNode's scheduler lifts timed ones.
            Via::Rcon => self
                .rcon(commands::ban(&player.name, reason))
                .await
                .map(drop),
            Via::Files => {
                let (player, reason) = (player.clone(), reason.map(str::to_owned));
                let now = nn_store::now_ms();
                self.edit_list(lists::BANNED_PLAYERS, move |list: &mut Vec<BanEntry>| {
                    lists::ban(list, &player, reason.as_deref(), SOURCE, now, expires_ms);
                })
                .await
            }
            Via::Unavailable(_) => Err(self.unavailable()),
        }
    }

    /// Lifts a ban. Returns a note when the server can't apply it until it
    /// restarts.
    pub async fn pardon(&self, player: &Profile) -> Result<Option<String>, ConnectError> {
        match &self.via {
            Via::Mgmt(client) => {
                let list = client.pardon(player).await.map_err(mgmt_error)?;
                // An offline-mode server can't match a player it has never
                // seen, so it can't remove them either; the entry is dropped
                // before the next start (see `drop_lifted_bans`).
                Ok(list
                    .iter()
                    .any(|b| b.player.uuid == player.uuid)
                    .then(|| {
                        format!(
                            "{} stays on the server's own ban list until it restarts: in offline mode it can't match a player it has never seen.",
                            player.name
                        )
                    }))
            }
            Via::Rcon => self
                .rcon(commands::pardon(&player.name))
                .await
                .map(|_| None),
            Via::Files => {
                let player = player.clone();
                self.edit_list(lists::BANNED_PLAYERS, move |list: &mut Vec<BanEntry>| {
                    lists::pardon(list, &player);
                })
                .await
                .map(|()| None)
            }
            Via::Unavailable(_) => Err(self.unavailable()),
        }
    }

    /// Removes bans NetherNode added that are no longer active (`active`
    /// holds the UUIDs and names of the bans still in force). Only for a
    /// stopped server.
    pub async fn drop_lifted_bans(
        &self,
        active: Vec<(Option<String>, String)>,
    ) -> Result<(), ConnectError> {
        self.edit_list(lists::BANNED_PLAYERS, move |list: &mut Vec<BanEntry>| {
            list.retain(|entry| {
                entry.source != SOURCE
                    || active.iter().any(|(uuid, name)| {
                        name.eq_ignore_ascii_case(&entry.name)
                            || uuid
                                .as_deref()
                                .and_then(normalize_uuid)
                                .is_some_and(|u| Some(u) == normalize_uuid(&entry.uuid))
                    })
            });
        })
        .await
    }

    /// Grants operator at `level` (1–4), or removes it with 0.
    pub async fn set_op(&self, player: &Profile, level: u8) -> Result<(), ConnectError> {
        match &self.via {
            Via::Mgmt(client) if level > 0 => {
                let operator = Operator {
                    player: player.clone(),
                    permission_level: Some(level),
                    bypasses_player_limit: None,
                };
                let list = client.op(&operator).await.map_err(mgmt_error)?;
                self.check_kept(list.iter().any(|o| o.player.uuid == player.uuid), player)
            }
            Via::Mgmt(client) => client.deop(player).await.map(drop).map_err(mgmt_error),
            Via::Rcon => self
                .rcon(commands::op(&player.name, level > 0))
                .await
                .map(drop),
            Via::Files => {
                let player = player.clone();
                self.edit_list(lists::OPS, move |list: &mut Vec<OpEntry>| {
                    if level > 0 {
                        lists::set_op(list, &player, level);
                    } else {
                        lists::remove_op(list, &player);
                    }
                })
                .await
            }
            Via::Unavailable(_) => Err(self.unavailable()),
        }
    }

    pub async fn set_allowlisted(
        &self,
        player: &Profile,
        allowed: bool,
    ) -> Result<(), ConnectError> {
        match &self.via {
            Via::Mgmt(client) if allowed => {
                let list = client
                    .allowlist_add(std::slice::from_ref(player))
                    .await
                    .map_err(mgmt_error)?;
                self.check_kept(list.iter().any(|p| p.uuid == player.uuid), player)
            }
            Via::Mgmt(client) => client
                .allowlist_remove(std::slice::from_ref(player))
                .await
                .map(drop)
                .map_err(mgmt_error),
            Via::Rcon => self
                .rcon(commands::whitelist(&player.name, allowed))
                .await
                .map(drop),
            Via::Files => {
                let player = player.clone();
                self.edit_list(lists::WHITELIST, move |list: &mut Vec<ListEntry>| {
                    if allowed {
                        lists::add_listed(list, &player);
                    } else {
                        lists::remove_listed(list, &player);
                    }
                })
                .await
            }
            Via::Unavailable(_) => Err(self.unavailable()),
        }
    }

    pub async fn set_allowlist_enabled(&self, enabled: bool) -> Result<(), ConnectError> {
        match &self.via {
            Via::Mgmt(client) => client
                .set_allowlist_enabled(enabled)
                .await
                .map(drop)
                .map_err(mgmt_error),
            Via::Rcon => self
                .rcon(Ok(commands::whitelist_enabled(enabled)))
                .await
                .map(drop),
            Via::Files => {
                self.set_property("white-list", if enabled { "true" } else { "false" })
                    .await
            }
            Via::Unavailable(_) => Err(self.unavailable()),
        }
    }

    fn running(&self) -> Result<(), ConnectError> {
        match &self.via {
            Via::Mgmt(_) | Via::Rcon => Ok(()),
            _ => Err(self.unavailable_or("That needs the server to be running.")),
        }
    }

    pub async fn game_mode(&self, player: &Profile, mode: GameMode) -> Result<(), ConnectError> {
        self.running()?;
        self.rcon(commands::gamemode(&player.name, mode))
            .await
            .map(drop)
    }

    pub async fn teleport(&self, player: &Profile, to: &Destination) -> Result<(), ConnectError> {
        self.running()?;
        self.rcon(commands::teleport(&player.name, to))
            .await
            .map(drop)
    }

    pub async fn give(&self, player: &Profile, item: &str, count: u32) -> Result<(), ConnectError> {
        self.running()?;
        self.rcon(commands::give(&player.name, item, count))
            .await
            .map(drop)
    }

    /// Who's online on this server, by name, from the management protocol
    /// or RCON. (The proxy's sessions are the primary source.)
    pub async fn online(&self) -> Vec<Profile> {
        match &self.via {
            Via::Mgmt(client) => client.players().await.unwrap_or_default(),
            _ => Vec::new(),
        }
    }
}
