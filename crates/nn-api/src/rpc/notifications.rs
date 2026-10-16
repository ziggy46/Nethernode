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
                ..Default::default()
            }
            .into(),
            last_error: status.last_error.unwrap_or_default(),
            last_sent_ms: status.last_sent_ms,
            update_check: crate::notices::update_check_on(&self.state).await,
            latest_version: crate::notices::latest_release()
                .map(|r| r.version)
                .unwrap_or_default(),
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
