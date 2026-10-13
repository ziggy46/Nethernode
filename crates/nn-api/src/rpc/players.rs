//! `PlayerService`: player lists, details, and actions. Moderators act on
//! players; bans on every server and unmasked IP addresses need an
//! administrator. Every action is audit-logged.

use std::collections::{BTreeMap, HashMap};

use connectrpc::{ConnectError, RequestContext, Response, ServiceRequest, ServiceResult};
use nn_auth::{AuthUser, Permission};
use nn_events::Event;
use nn_files::{EDITOR_LIMIT, FileError, RelPath};
use nn_players::commands::{Destination, GameMode};
use nn_players::profile::Profile;
use nn_players::{nbt, playerdata};
use nn_store::{NewPlayerBan, PlayerBan};
use serde_json::json;

use crate::AppState;
use crate::files::blocking;
use crate::players::backend::{Ctx, Via};
use crate::players::resolve;
use crate::proto::nethernode::v1 as pb;
use crate::rpc::{audit, require, store_error};

pub(crate) struct PlayerRpc {
    state: AppState,
}

impl PlayerRpc {
    pub(crate) fn new(state: AppState) -> Self {
        Self { state }
    }

    fn changed(&self, server_id: Option<&str>) {
        self.state.events.publish(Event::PlayersChanged {
            server_id: server_id.map(str::to_owned),
        });
    }

    async fn record(
        &self,
        user: &AuthUser,
        action: &str,
        player: &Profile,
        details: serde_json::Value,
    ) {
        let mut details = details;
        details["name"] = json!(player.name);
        audit(&self.state, user, action, ("player", &player.uuid), details).await;
    }
}

/// Last-known IPs are shown to administrators; others see a masked form.
fn mask_ip(ip: &str, full: bool) -> String {
    if full || ip.is_empty() {
        return ip.to_owned();
    }
    match ip.parse::<std::net::IpAddr>() {
        Ok(std::net::IpAddr::V4(v4)) => {
            let [a, b, c, _] = v4.octets();
            format!("{a}.{b}.{c}.•••")
        }
        Ok(std::net::IpAddr::V6(v6)) => {
            let s = v6.segments();
            format!("{:x}:{:x}::•••", s[0], s[1])
        }
        Err(_) => "•••".into(),
    }
}

/// A reason on one line, at most 200 characters.
fn reason(text: &str) -> Option<String> {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let flat: String = flat
        .chars()
        .filter(|c| !c.is_control() && *c != '§')
        .take(200)
        .collect();
    (!flat.is_empty()).then_some(flat)
}

fn player_ref(
    view: &buffa::MessageFieldView<pb::__buffa::view::PlayerRefView<'_>>,
) -> Result<(String, Option<String>), ConnectError> {
    let p = view
        .as_option()
        .ok_or_else(|| ConnectError::invalid_argument("choose a player"))?;
    Ok((p.name.to_owned(), p.uuid.map(str::to_owned)))
}

async fn resolve_in(
    ctx: &Ctx<'_>,
    player: &(String, Option<String>),
) -> Result<Profile, ConnectError> {
    // A UUID from a list or session only identifies the player on servers
    // in the same mode; offline-mode servers resolve by name.
    let uuid = if ctx.online_mode {
        player.1.as_deref()
    } else {
        None
    };
    resolve::profile(ctx, &player.0, uuid).await
}

fn ban_info(
    reason: Option<String>,
    expires_ms: Option<i64>,
    source: String,
    nn: Option<&PlayerBan>,
    full_ip: bool,
) -> pb::PlayerBanInfo {
    pb::PlayerBanInfo {
        reason: reason
            .or_else(|| nn.and_then(|b| b.reason.clone()))
            .unwrap_or_default(),
        expires_ms: expires_ms.or_else(|| nn.and_then(|b| b.expires_at)),
        global: nn.is_some_and(|b| b.server_id.is_none()),
        ip: nn
            .and_then(|b| b.ip.as_deref())
            .map(|ip| mask_ip(ip, full_ip))
            .unwrap_or_default(),
        source,
        ..Default::default()
    }
}

impl PlayerRpc {
    /// Rows for one server (with its lists) or all servers.
    async fn rows(
        &self,
        server_id: Option<&str>,
        full_ip: bool,
    ) -> Result<(Vec<pb::PlayerRow>, Option<pb::Capabilities>, bool), ConnectError> {
        let store = self.state.auth.store();
        let summaries = store
            .list_player_summaries(server_id)
            .await
            .map_err(store_error)?;
        let sessions = store
            .online_player_sessions(server_id)
            .await
            .map_err(store_error)?;
        let nn_bans = store
            .active_player_bans(server_id)
            .await
            .map_err(store_error)?;
        let nn_ban = |uuid: &str| nn_bans.iter().find(|b| b.uuid.as_deref() == Some(uuid));

        let mut rows: BTreeMap<String, pb::PlayerRow> = BTreeMap::new();
        for s in summaries {
            rows.insert(
                s.uuid.clone(),
                pb::PlayerRow {
                    uuid: s.uuid,
                    name: s.name,
                    first_seen_ms: s.first_seen,
                    last_seen_ms: s.last_seen,
                    playtime_ms: s.playtime_ms,
                    last_ip: mask_ip(s.last_ip.as_deref().unwrap_or_default(), full_ip),
                    has_notes: s.notes.is_some_and(|n| !n.is_empty()),
                    ..Default::default()
                },
            );
        }
        let entry = |rows: &mut BTreeMap<String, pb::PlayerRow>, p: &Profile| {
            rows.entry(p.uuid.clone())
                .or_insert_with(|| pb::PlayerRow {
                    uuid: p.uuid.clone(),
                    name: p.name.clone(),
                    ..Default::default()
                })
                .uuid
                .clone()
        };

        let mut capabilities = None;
        let mut allowlist_enabled = false;
        if let Some(id) = server_id {
            let ctx = Ctx::open(&self.state, id).await?;
            let lists = ctx.lists().await?;
            allowlist_enabled = lists.allowlist_enabled;
            for (p, level) in &lists.ops {
                let key = entry(&mut rows, p);
                rows.get_mut(&key).unwrap().op_level = u32::from(*level);
            }
            for p in &lists.allowlist {
                let key = entry(&mut rows, p);
                rows.get_mut(&key).unwrap().allowlisted = true;
            }
            for (p, why, expires, source) in lists.bans {
                let key = entry(&mut rows, &p);
                let info = ban_info(why, expires, source, nn_ban(&p.uuid), full_ip);
                rows.get_mut(&key).unwrap().ban = info.into();
            }
            for p in ctx.online().await {
                let key = entry(&mut rows, &p);
                rows.get_mut(&key).unwrap().online = true;
            }
            capabilities = Some(ctx.capabilities());
        }
        // NetherNode's own bans not (yet) in a server's list: global bans
        // waiting for a server to start, or the all-servers view.
        for ban in &nn_bans {
            let Some(uuid) = &ban.uuid else { continue };
            let key = entry(
                &mut rows,
                &Profile {
                    uuid: uuid.clone(),
                    name: ban.name.clone(),
                },
            );
            let row = rows.get_mut(&key).unwrap();
            if row.ban.as_option().is_none() {
                row.ban = ban_info(None, None, "NetherNode".into(), Some(ban), full_ip).into();
            }
        }
        let mut online: HashMap<String, Vec<String>> = HashMap::new();
        for s in sessions {
            let Some(uuid) = s.uuid else { continue };
            online.entry(uuid).or_default().push(s.server_id);
        }
        let mut out: Vec<pb::PlayerRow> = rows
            .into_values()
            .map(|mut row| {
                if let Some(servers) = online.remove(&row.uuid) {
                    row.online = true;
                    row.online_server_ids = servers;
                }
                row
            })
            .collect();
        out.sort_by(|a, b| {
            b.online
                .cmp(&a.online)
                .then(b.last_seen_ms.cmp(&a.last_seen_ms))
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
        Ok((out, capabilities, allowlist_enabled))
    }

    /// Applies `work` on one server, or on every server for global actions,
    /// collecting per-server problems as warnings.
    async fn each_server<F>(
        &self,
        server_id: Option<&str>,
        player: &(String, Option<String>),
        work: F,
    ) -> Result<(Profile, Vec<String>), ConnectError>
    where
        F: for<'c> Fn(
            &'c Ctx<'c>,
            &'c Profile,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<(), ConnectError>> + Send + 'c>,
        >,
    {
        if let Some(id) = server_id {
            let ctx = Ctx::open(&self.state, id).await?;
            let profile = resolve_in(&ctx, player).await?;
            work(&ctx, &profile).await?;
            return Ok((profile, Vec::new()));
        }
        let servers = self
            .state
            .auth
            .store()
            .list_servers()
            .await
            .map_err(store_error)?;
        let mut warnings = Vec::new();
        let mut first: Option<Profile> = None;
        for server in servers {
            let ctx = Ctx::open(&self.state, &server.id).await?;
            if let Via::Unavailable(why) = &ctx.via {
                warnings.push(format!(
                    "{}: {why} It will be applied when the server next starts.",
                    server.name
                ));
                continue;
            }
            match resolve_in(&ctx, player).await {
                Ok(profile) => {
                    if let Err(e) = work(&ctx, &profile).await {
                        warnings.push(format!(
                            "{}: {}",
                            server.name,
                            e.message.unwrap_or_default()
                        ));
                    }
                    if ctx.online_mode || first.is_none() {
                        first = Some(profile);
                    }
                }
                Err(e) => warnings.push(format!(
                    "{}: {}",
                    server.name,
                    e.message.unwrap_or_default()
                )),
            }
        }
        let profile = match first {
            Some(p) => p,
            // No servers: still record the ban against the Mojang account.
            None => {
                let looked_up = self
                    .state
                    .players
                    .lookup(self.state.auth.store(), &player.0)
                    .await?;
                looked_up.ok_or_else(|| {
                    ConnectError::not_found(format!("No Minecraft account is named {}.", player.0))
                })?
            }
        };
        Ok((profile, warnings))
    }
}

#[allow(refining_impl_trait)]
impl pb::PlayerService for PlayerRpc {
    async fn list_players(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::ListPlayersRequest>,
    ) -> ServiceResult<pb::ListPlayersResponse> {
        let user = require(&ctx, Permission::View)?;
        let full_ip = user.role.can(Permission::ManageServers);
        let (players, capabilities, allowlist_enabled) =
            self.rows(request.server_id, full_ip).await?;
        Response::ok(pb::ListPlayersResponse {
            players,
            capabilities: capabilities.into(),
            allowlist_enabled,
            ..Default::default()
        })
    }

    async fn get_player(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::GetPlayerRequest>,
    ) -> ServiceResult<pb::GetPlayerResponse> {
        let user = require(&ctx, Permission::View)?;
        let full_ip = user.role.can(Permission::ManageServers);
        let store = self.state.auth.store();
        let (rows, capabilities, _) = self.rows(request.server_id, full_ip).await?;
        let row = rows
            .into_iter()
            .find(|r| r.uuid == request.uuid)
            .ok_or_else(|| ConnectError::not_found("NetherNode doesn't know this player"))?;
        let names: HashMap<String, String> = store
            .list_servers()
            .await
            .map_err(store_error)?
            .into_iter()
            .map(|s| (s.id, s.name))
            .collect();
        let sessions = store
            .player_session_history(request.uuid, request.server_id, 50)
            .await
            .map_err(store_error)?
            .into_iter()
            .map(|s| pb::PlayerSessionRecord {
                server_name: names.get(&s.server_id).cloned().unwrap_or_default(),
                server_id: s.server_id,
                joined_ms: s.joined_at,
                left_ms: s.left_at,
                ip: mask_ip(s.ip.as_deref().unwrap_or_default(), full_ip),
                ..Default::default()
            })
            .collect();
        let notes = store
            .player_notes(request.uuid)
            .await
            .map_err(store_error)?
            .unwrap_or_default();
        Response::ok(pb::GetPlayerResponse {
            player: row.into(),
            notes,
            sessions,
            capabilities: capabilities.into(),
            ..Default::default()
        })
    }

    async fn get_player_data(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::GetPlayerDataRequest>,
    ) -> ServiceResult<pb::GetPlayerDataResponse> {
        require(&ctx, Permission::View)?;
        let uuid = nn_players::profile::normalize_uuid(request.uuid)
            .ok_or_else(|| ConnectError::invalid_argument("not a player UUID"))?;
        let (_, files) = crate::files::open(&self.state, request.server_id).await?;
        let found = blocking(move || {
            let read = |path: &str| -> Result<Option<(Vec<u8>, i64)>, FileError> {
                let rel = RelPath::parse(path)?;
                match files.read(&rel, EDITOR_LIMIT) {
                    Ok(content) => Ok(Some((content.data, content.modified_ms))),
                    Err(FileError::NotFound(_)) => Ok(None),
                    Err(e) => Err(e),
                }
            };
            let level = match read("server.properties")? {
                Some((bytes, _)) => nn_files::Properties::parse(&String::from_utf8_lossy(&bytes))
                    .get("level-name")
                    .filter(|l| nn_files::validate_name(l).is_ok())
                    .unwrap_or("world")
                    .to_owned(),
                None => "world".to_owned(),
            };
            let mut data = None;
            for path in playerdata::data_paths(&level, &uuid) {
                if let Some(found) = read(&path)? {
                    data = Some(found);
                    break;
                }
            }
            let mut stats = Vec::new();
            for path in playerdata::stats_paths(&level, &uuid) {
                if let Some((bytes, _)) = read(&path)? {
                    stats = playerdata::parse_stats(&bytes);
                    break;
                }
            }
            Ok((data, stats))
        })
        .await?;
        let (Some((bytes, saved_ms)), stats) = found else {
            return Response::ok(pb::GetPlayerDataResponse::default());
        };
        let root = nbt::read_gzip(&bytes).map(|(_, root)| root).map_err(|e| {
            ConnectError::failed_precondition(format!("The player's data file can't be read: {e}"))
        })?;
        let data = playerdata::parse(&root);
        let item = |i: &playerdata::Item| pb::ItemStack {
            slot: i.slot,
            id: i.id.clone(),
            count: i.count,
            custom_name: i.custom_name.clone().unwrap_or_default(),
            enchantments: i
                .enchantments
                .iter()
                .map(|(id, level)| pb::Enchantment {
                    id: id.clone(),
                    level: *level,
                    ..Default::default()
                })
                .collect(),
            damage: i.damage.unwrap_or(0),
            ..Default::default()
        };
        let e = &data.equipment;
        Response::ok(pb::GetPlayerDataResponse {
            found: true,
            saved_ms,
            position: data.position.map(|p| p.to_vec()).unwrap_or_default(),
            dimension: data.dimension.clone().unwrap_or_default(),
            health: data.health.unwrap_or(0.0),
            food: data.food.unwrap_or(0),
            xp_level: data.xp_level.unwrap_or(0),
            xp_progress: data.xp_progress.unwrap_or(0.0),
            game_mode: data.game_mode.unwrap_or(-1),
            selected_slot: data.selected_slot.unwrap_or(0),
            inventory: data.inventory.iter().map(item).collect(),
            ender_chest: data.ender_chest.iter().map(item).collect(),
            head: e.head.as_ref().map(item).into(),
            chest: e.chest.as_ref().map(item).into(),
            legs: e.legs.as_ref().map(item).into(),
            feet: e.feet.as_ref().map(item).into(),
            offhand: e.offhand.as_ref().map(item).into(),
            stats: stats
                .into_iter()
                .map(|s| pb::StatEntry {
                    category: s.category,
                    key: s.key,
                    value: s.value,
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        })
    }

    async fn resolve_player(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::ResolvePlayerRequest>,
    ) -> ServiceResult<pb::ResolvePlayerResponse> {
        require(&ctx, Permission::ManagePlayers)?;
        let server = Ctx::open(&self.state, request.server_id).await?;
        let profile = resolve::profile(&server, request.name, None).await?;
        Response::ok(pb::ResolvePlayerResponse {
            uuid: profile.uuid,
            name: profile.name,
            offline: !server.online_mode,
            ..Default::default()
        })
    }

    async fn kick_player(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::KickPlayerRequest>,
    ) -> ServiceResult<pb::KickPlayerResponse> {
        let user = require(&ctx, Permission::ManagePlayers)?.clone();
        let player = player_ref(&request.player)?;
        let message = reason(request.message).unwrap_or_else(|| "Kicked by an operator".into());
        let server = Ctx::open(&self.state, request.server_id).await?;
        let profile = resolve_in(&server, &player).await?;
        server.kick(&profile, &message).await?;
        self.record(
            &user,
            "player.kick",
            &profile,
            json!({ "server_id": request.server_id, "message": message }),
        )
        .await;
        self.changed(Some(request.server_id));
        Response::ok(pb::KickPlayerResponse::default())
    }

    async fn ban_player(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::BanPlayerRequest>,
    ) -> ServiceResult<pb::BanPlayerResponse> {
        let permission = if request.server_id.is_some() {
            Permission::ManagePlayers
        } else {
            Permission::ManageServers
        };
        let user = require(&ctx, permission)?.clone();
        let player = player_ref(&request.player)?;
        let why = reason(request.reason);
        let expires_at = match request.duration_seconds {
            None => None,
            Some(secs) if (60..=10 * 365 * 86_400).contains(&secs) => {
                Some(nn_store::now_ms() + secs * 1000)
            }
            Some(_) => {
                return Err(ConnectError::invalid_argument(
                    "Ban length must be between a minute and ten years.",
                ));
            }
        };
        let (profile, warnings) = self
            .each_server(request.server_id, &player, |ctx, profile| {
                let why = why.clone();
                Box::pin(async move { ctx.ban(profile, why.as_deref(), expires_at).await })
            })
            .await?;
        let store = self.state.auth.store();
        let ip = if request.ban_ip {
            let known = store
                .player_summary(&profile.uuid)
                .await
                .map_err(store_error)?
                .and_then(|s| s.last_ip);
            Some(known.ok_or_else(|| {
                ConnectError::failed_precondition(format!(
                    "{} was banned, but no IP address is known for them yet (they haven't joined through NetherNode).",
                    profile.name
                ))
            })?)
        } else {
            None
        };
        store
            .insert_player_ban(NewPlayerBan {
                server_id: request.server_id,
                uuid: Some(&profile.uuid),
                name: &profile.name,
                ip: ip.as_deref(),
                reason: why.as_deref(),
                expires_at,
                created_by: Some(&user.id),
            })
            .await
            .map_err(store_error)?;
        self.state.players.ip_bans.reload(store).await;
        self.record(
            &user,
            "player.ban",
            &profile,
            json!({ "server_id": request.server_id, "reason": why, "expires_at": expires_at, "ip": ip.is_some() }),
        )
        .await;
        self.changed(request.server_id);
        Response::ok(pb::BanPlayerResponse {
            warnings,
            ..Default::default()
        })
    }

    async fn unban_player(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::UnbanPlayerRequest>,
    ) -> ServiceResult<pb::UnbanPlayerResponse> {
        let permission = if request.server_id.is_some() {
            Permission::ManagePlayers
        } else {
            Permission::ManageServers
        };
        let user = require(&ctx, permission)?.clone();
        let player = player_ref(&request.player)?;
        let (profile, warnings) = self
            .each_server(request.server_id, &player, |ctx, profile| {
                Box::pin(ctx.pardon(profile))
            })
            .await?;
        let store = self.state.auth.store();
        store
            .lift_player_bans_for(&profile.uuid, request.server_id)
            .await
            .map_err(store_error)?;
        self.state.players.ip_bans.reload(store).await;
        self.record(
            &user,
            "player.unban",
            &profile,
            json!({ "server_id": request.server_id }),
        )
        .await;
        self.changed(request.server_id);
        Response::ok(pb::UnbanPlayerResponse {
            warnings,
            ..Default::default()
        })
    }

    async fn set_operator(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::SetOperatorRequest>,
    ) -> ServiceResult<pb::SetOperatorResponse> {
        let user = require(&ctx, Permission::ManagePlayers)?.clone();
        let level = u8::try_from(request.level)
            .ok()
            .filter(|l| *l <= 4)
            .ok_or_else(|| {
                ConnectError::invalid_argument("Operator level must be 1–4, or 0 to remove.")
            })?;
        let player = player_ref(&request.player)?;
        let server = Ctx::open(&self.state, request.server_id).await?;
        let profile = resolve_in(&server, &player).await?;
        server.set_op(&profile, level).await?;
        let action = if level > 0 {
            "player.op"
        } else {
            "player.deop"
        };
        self.record(
            &user,
            action,
            &profile,
            json!({ "server_id": request.server_id, "level": level }),
        )
        .await;
        self.changed(Some(request.server_id));
        Response::ok(pb::SetOperatorResponse::default())
    }

    async fn set_allowlisted(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::SetAllowlistedRequest>,
    ) -> ServiceResult<pb::SetAllowlistedResponse> {
        let user = require(&ctx, Permission::ManagePlayers)?.clone();
        let player = player_ref(&request.player)?;
        let server = Ctx::open(&self.state, request.server_id).await?;
        let profile = resolve_in(&server, &player).await?;
        server.set_allowlisted(&profile, request.allowed).await?;
        let action = if request.allowed {
            "player.allowlist_add"
        } else {
            "player.allowlist_remove"
        };
        self.record(
            &user,
            action,
            &profile,
            json!({ "server_id": request.server_id }),
        )
        .await;
        self.changed(Some(request.server_id));
        Response::ok(pb::SetAllowlistedResponse::default())
    }

    async fn set_allowlist_enabled(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::SetAllowlistEnabledRequest>,
    ) -> ServiceResult<pb::SetAllowlistEnabledResponse> {
        let user = require(&ctx, Permission::ManagePlayers)?.clone();
        let server = Ctx::open(&self.state, request.server_id).await?;
        server.set_allowlist_enabled(request.enabled).await?;
        audit(
            &self.state,
            &user,
            "server.allowlist_enabled",
            ("server", request.server_id),
            json!({ "enabled": request.enabled }),
        )
        .await;
        self.changed(Some(request.server_id));
        Response::ok(pb::SetAllowlistEnabledResponse::default())
    }

    async fn import_allowlist(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::ImportAllowlistRequest>,
    ) -> ServiceResult<pb::ImportAllowlistResponse> {
        let user = require(&ctx, Permission::ManagePlayers)?.clone();
        let names: Vec<String> = request
            .names
            .iter()
            .map(|n| n.trim().to_owned())
            .filter(|n| !n.is_empty())
            .collect();
        if names.len() > 500 {
            return Err(ConnectError::invalid_argument(
                "Import up to 500 names at a time.",
            ));
        }
        let server = Ctx::open(&self.state, request.server_id).await?;
        let (mut added, mut failed) = (Vec::new(), Vec::new());
        for name in names {
            match resolve::profile(&server, &name, None).await {
                Ok(profile) => match server.set_allowlisted(&profile, true).await {
                    Ok(()) => added.push(profile.name),
                    Err(e) => failed.push(format!("{name}: {}", e.message.unwrap_or_default())),
                },
                Err(e) => failed.push(format!("{name}: {}", e.message.unwrap_or_default())),
            }
        }
        audit(
            &self.state,
            &user,
            "player.allowlist_import",
            ("server", request.server_id),
            json!({ "added": added, "failed": failed.len() }),
        )
        .await;
        self.changed(Some(request.server_id));
        Response::ok(pb::ImportAllowlistResponse {
            added,
            failed,
            ..Default::default()
        })
    }

    async fn set_game_mode(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::SetGameModeRequest>,
    ) -> ServiceResult<pb::SetGameModeResponse> {
        let user = require(&ctx, Permission::ManagePlayers)?.clone();
        let mode = match request.mode.as_known() {
            Some(pb::GameMode::GAME_MODE_SURVIVAL) => GameMode::Survival,
            Some(pb::GameMode::GAME_MODE_CREATIVE) => GameMode::Creative,
            Some(pb::GameMode::GAME_MODE_ADVENTURE) => GameMode::Adventure,
            Some(pb::GameMode::GAME_MODE_SPECTATOR) => GameMode::Spectator,
            _ => return Err(ConnectError::invalid_argument("choose a game mode")),
        };
        let player = player_ref(&request.player)?;
        let server = Ctx::open(&self.state, request.server_id).await?;
        let profile = resolve_in(&server, &player).await?;
        server.game_mode(&profile, mode).await?;
        self.record(
            &user,
            "player.gamemode",
            &profile,
            json!({ "server_id": request.server_id, "mode": mode.as_str() }),
        )
        .await;
        Response::ok(pb::SetGameModeResponse::default())
    }

    async fn teleport_player(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::TeleportPlayerRequest>,
    ) -> ServiceResult<pb::TeleportPlayerResponse> {
        use pb::__buffa::view::oneof::teleport_player_request::Destination as To;
        let user = require(&ctx, Permission::ManagePlayers)?.clone();
        let to = match &request.destination {
            Some(To::ToPlayer(name)) => Destination::Player((*name).to_owned()),
            Some(To::ToCoordinates(c)) => Destination::Coordinates(c.x, c.y, c.z),
            None => return Err(ConnectError::invalid_argument("choose where to teleport")),
        };
        let player = player_ref(&request.player)?;
        let server = Ctx::open(&self.state, request.server_id).await?;
        let profile = resolve_in(&server, &player).await?;
        server.teleport(&profile, &to).await?;
        let target = match &to {
            Destination::Player(name) => json!(name),
            Destination::Coordinates(x, y, z) => json!([x, y, z]),
        };
        self.record(
            &user,
            "player.teleport",
            &profile,
            json!({ "server_id": request.server_id, "to": target }),
        )
        .await;
        Response::ok(pb::TeleportPlayerResponse::default())
    }

    async fn give_item(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::GiveItemRequest>,
    ) -> ServiceResult<pb::GiveItemResponse> {
        let user = require(&ctx, Permission::ManagePlayers)?.clone();
        let player = player_ref(&request.player)?;
        let item = request.item.trim();
        let item = if item.contains(':') {
            item.to_owned()
        } else {
            format!("minecraft:{item}")
        };
        let server = Ctx::open(&self.state, request.server_id).await?;
        let profile = resolve_in(&server, &player).await?;
        server.give(&profile, &item, request.count).await?;
        self.record(
            &user,
            "player.give",
            &profile,
            json!({ "server_id": request.server_id, "item": item, "count": request.count }),
        )
        .await;
        Response::ok(pb::GiveItemResponse::default())
    }

    async fn set_player_notes(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::SetPlayerNotesRequest>,
    ) -> ServiceResult<pb::SetPlayerNotesResponse> {
        let user = require(&ctx, Permission::ManagePlayers)?.clone();
        let notes = request.notes.trim();
        if notes.chars().count() > 4000 {
            return Err(ConnectError::invalid_argument(
                "Notes can be up to 4,000 characters.",
            ));
        }
        let store = self.state.auth.store();
        let saved = store
            .set_player_notes(request.uuid, (!notes.is_empty()).then_some(notes))
            .await
            .map_err(store_error)?;
        if !saved {
            return Err(ConnectError::not_found(
                "NetherNode hasn't seen this player join yet.",
            ));
        }
        audit(
            &self.state,
            &user,
            "player.notes",
            ("player", request.uuid),
            json!({}),
        )
        .await;
        Response::ok(pb::SetPlayerNotesResponse::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn masks() {
        assert_eq!(mask_ip("203.0.113.7", false), "203.0.113.•••");
        assert_eq!(mask_ip("203.0.113.7", true), "203.0.113.7");
        assert_eq!(mask_ip("2001:db8::1", false), "2001:db8::•••");
        assert_eq!(mask_ip("", false), "");
    }

    #[test]
    fn reasons() {
        assert_eq!(
            reason("  griefing\nthe   spawn §c "),
            Some("griefing the spawn c".into())
        );
        assert_eq!(reason("   "), None);
    }
}
