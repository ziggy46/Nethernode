//! NotificationService: Discord webhook settings.

use connectrpc::{ConnectError, RequestContext, Response, ServiceRequest, ServiceResult};
use nn_auth::Permission;
use serde_json::json;

use crate::AppState;
use crate::notify::Toggles;
use crate::proto::nethernode::v1 as pb;
use crate::rpc::{audit, require};

pub(crate) struct NotificationRpc {
    state: AppState,
}

impl NotificationRpc {
    pub(crate) fn new(state: AppState) -> Self {
        Self { state }
    }
}

#[allow(refining_impl_trait)]
impl pb::NotificationService for NotificationRpc {
    async fn get_notifications(
        &self,
        ctx: RequestContext,
        _request: ServiceRequest<'_, pb::GetNotificationsRequest>,
    ) -> ServiceResult<pb::GetNotificationsResponse> {
        require(&ctx, Permission::ManageSettings)?;
        let status = self.state.notifier.status();
        Response::ok(pb::GetNotificationsResponse {
            webhook_hint: status.webhook_hint,
            events: pb::NotificationEvents {
                start_stop: status.toggles.start_stop,
                crash: status.toggles.crash,
                player_join: status.toggles.player_join,
                backup_failed: status.toggles.backup_failed,
                modpack_updates: status.toggles.modpack_updates,
                resource_alerts: status.toggles.resource_alerts,
                ..Default::default()
            }
            .into(),
            last_error: status.last_error.unwrap_or_default(),
            last_sent_ms: status.last_sent_ms,
            update_check: crate::notices::update_check_on(&self.state).await,
            latest_version: crate::notices::latest_release()
                .map(|r| r.version)
                .unwrap_or_default(),
            current_version: crate::VERSION.to_owned(),
            update_available: crate::notices::latest_release()
                .is_some_and(|r| crate::notices::offered(&r)),
            update_is_switch: crate::notices::latest_release()
                .is_some_and(|r| crate::notices::is_switch(&r)),
            update_channel: crate::notices::channel(&self.state)
                .await
                .as_str()
                .to_owned(),
            ..Default::default()
        })
    }

    async fn check_for_update(
        &self,
        ctx: RequestContext,
        _request: ServiceRequest<'_, pb::CheckForUpdateRequest>,
    ) -> ServiceResult<pb::CheckForUpdateResponse> {
        require(&ctx, Permission::ManageSettings)?;
        let release = crate::notices::check_regardless(&self.state)
            .await
            .map_err(|e| {
                ConnectError::unavailable(format!("Couldn't reach GitHub to check: {e}"))
            })?;
        Response::ok(pb::CheckForUpdateResponse {
            update_available: crate::notices::offered(&release),
            update_is_switch: crate::notices::is_switch(&release),
            latest_version: release.version,
            ..Default::default()
        })
    }

    async fn update_panel(
        &self,
        ctx: RequestContext,
        _request: ServiceRequest<'_, pb::UpdatePanelRequest>,
    ) -> ServiceResult<pb::UpdatePanelResponse> {
        let user = require(&ctx, Permission::ManageSettings)?.clone();
        let channel = crate::notices::channel(&self.state).await;
        let release = match crate::notices::latest_release().filter(|r| r.channel == channel) {
            Some(release) => release,
            None => crate::notices::check_regardless(&self.state)
                .await
                .map_err(|e| {
                    ConnectError::unavailable(format!("Couldn't reach GitHub to check: {e}"))
                })?,
        };
        if !crate::notices::offered(&release) {
            return Err(ConnectError::failed_precondition(
                "This panel already runs the newest release.",
            ));
        }
        let next =
            nn_node::update::prepare(self.state.nodes.local(), crate::VERSION, &release.version)
                .await
                .map_err(|e| {
                    ConnectError::failed_precondition(format!("The update couldn't start: {e}"))
                })?;
        audit(
            &self.state,
            &user,
            "settings.update_panel",
            ("settings", "updates"),
            json!({ "from": crate::VERSION, "to": release.version, "channel": release.channel.as_str() }),
        )
        .await;
        tracing::info!(to = %release.version, "updating the panel");
        // Answer first; then hand over to the new version.
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            match next {
                nn_node::update::Next::Exec(exe, _) => {
                    let error = nn_node::update::exec(&exe);
                    tracing::error!(%error, "couldn't restart as the new version; exiting");
                    std::process::exit(1);
                }
                nn_node::update::Next::Exit(_) => {
                    tracing::info!("the new panel container has taken over; exiting");
                    std::process::exit(0);
                }
                nn_node::update::Next::Stay(_) => {}
            }
        });
        Response::ok(pb::UpdatePanelResponse {
            version: release.version,
            ..Default::default()
        })
    }

    async fn set_update_check(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::SetUpdateCheckRequest>,
    ) -> ServiceResult<pb::SetUpdateCheckResponse> {
        let user = require(&ctx, Permission::ManageSettings)?.clone();
        self.state
            .auth
            .store()
            .set_setting(crate::notices::UPDATE_CHECK_SETTING, &request.enabled)
            .await
            .map_err(crate::rpc::store_error)?;
        audit(
            &self.state,
            &user,
            "settings.update_check",
            ("settings", "updates"),
            json!({ "enabled": request.enabled }),
        )
        .await;
        let state = self.state.clone();
        tokio::spawn(async move { crate::notices::check_now(&state).await });
        Response::ok(pb::SetUpdateCheckResponse::default())
    }

    async fn set_update_channel(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::SetUpdateChannelRequest>,
    ) -> ServiceResult<pb::SetUpdateChannelResponse> {
        use crate::notices::Channel;
        let user = require(&ctx, Permission::ManageSettings)?.clone();
        let channel = match request.channel {
            "stable" => Channel::Stable,
            "edge" => Channel::Edge,
            _ => return Err(ConnectError::invalid_argument("choose stable or edge")),
        };
        self.state
            .auth
            .store()
            .set_setting(crate::notices::CHANNEL_SETTING, &channel)
            .await
            .map_err(crate::rpc::store_error)?;
        audit(
            &self.state,
            &user,
            "settings.update_channel",
            ("settings", "updates"),
            json!({ "channel": channel.as_str() }),
        )
        .await;
        // What was found before was for the other channel.
        crate::notices::forget_latest();
        crate::notices::check_now(&self.state).await;
        Response::ok(pb::SetUpdateChannelResponse::default())
    }

    async fn update_notifications(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::UpdateNotificationsRequest>,
    ) -> ServiceResult<pb::UpdateNotificationsResponse> {
        let user = require(&ctx, Permission::ManageSettings)?.clone();
        let events = request.events.as_option();
        let toggles = Toggles {
            start_stop: events.is_some_and(|e| e.start_stop),
            crash: events.is_some_and(|e| e.crash),
            player_join: events.is_some_and(|e| e.player_join),
            backup_failed: events.is_some_and(|e| e.backup_failed),
            modpack_updates: events.is_some_and(|e| e.modpack_updates),
            resource_alerts: events.is_some_and(|e| e.resource_alerts),
        };
        self.state
            .notifier
            .save(
                self.state.auth.store(),
                self.state.content.secrets(),
                request.webhook_url,
                toggles,
            )
            .await?;
        let webhook = match request.webhook_url.map(str::trim) {
            None => "kept",
            Some("") => "removed",
            Some(_) => "changed",
        };
        audit(
            &self.state,
            &user,
            "settings.notifications",
            ("settings", "notifications"),
            json!({ "webhook": webhook, "events": serde_json::to_value(toggles).unwrap_or_default() }),
        )
        .await;
        Response::ok(pb::UpdateNotificationsResponse::default())
    }

    async fn send_test_notification(
        &self,
        ctx: RequestContext,
        _request: ServiceRequest<'_, pb::SendTestNotificationRequest>,
    ) -> ServiceResult<pb::SendTestNotificationResponse> {
        require(&ctx, Permission::ManageSettings)?;
        self.state
            .notifier
            .send_test()
            .await
            .map_err(ConnectError::failed_precondition)?;
        Response::ok(pb::SendTestNotificationResponse::default())
    }
}
