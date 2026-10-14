//! ScheduleService.

use std::collections::HashMap;

use connectrpc::{ConnectError, RequestContext, Response, ServiceRequest, ServiceResult};
use nn_auth::Permission;
use nn_store::{Schedule, ScheduleInput};
use serde_json::json;

use crate::AppState;
use crate::proto::nethernode::v1 as pb;
use crate::rpc::{audit, require, store_error};
use crate::schedules::{Payload, next_runs, parse_cron, parse_tz, runner};

pub(crate) struct ScheduleRpc {
    state: AppState,
}

impl ScheduleRpc {
    pub(crate) fn new(state: AppState) -> Self {
        Self { state }
    }

    async fn load(&self, id: &str) -> Result<Schedule, ConnectError> {
        self.state
            .auth
            .store()
            .find_schedule(id)
            .await
            .map_err(store_error)?
            .ok_or_else(|| ConnectError::not_found("no such schedule"))
    }

    async fn names(&self) -> Result<HashMap<String, String>, ConnectError> {
        Ok(self
            .state
            .auth
            .store()
            .list_servers()
            .await
            .map_err(store_error)?
            .into_iter()
            .map(|s| (s.id, s.name))
            .collect())
    }
}

fn action_name(a: Option<pb::ScheduleAction>) -> Option<&'static str> {
    use pb::ScheduleAction as A;
    Some(match a? {
        A::SCHEDULE_ACTION_RESTART => "restart",
        A::SCHEDULE_ACTION_BACKUP => "backup",
        A::SCHEDULE_ACTION_COMMAND => "command",
        A::SCHEDULE_ACTION_BROADCAST => "broadcast",
        A::SCHEDULE_ACTION_START => "start",
        A::SCHEDULE_ACTION_STOP => "stop",
        A::SCHEDULE_ACTION_UNSPECIFIED => return None,
    })
}

fn proto_action(a: &str) -> pb::ScheduleAction {
    use pb::ScheduleAction as A;
    match a {
        "restart" => A::SCHEDULE_ACTION_RESTART,
        "backup" => A::SCHEDULE_ACTION_BACKUP,
        "command" => A::SCHEDULE_ACTION_COMMAND,
        "broadcast" => A::SCHEDULE_ACTION_BROADCAST,
        "start" => A::SCHEDULE_ACTION_START,
        "stop" => A::SCHEDULE_ACTION_STOP,
        _ => A::SCHEDULE_ACTION_UNSPECIFIED,
    }
}

fn proto_schedule(s: Schedule, names: &HashMap<String, String>) -> pb::Schedule {
    let payload: Payload = serde_json::from_str(&s.payload_json).unwrap_or_default();
    pb::Schedule {
        server_name: names.get(&s.server_id).cloned().unwrap_or_default(),
        action: proto_action(&s.action).into(),
        text: payload.text,
        warn_minutes: payload.warn_minutes,
        keep_daily: payload.keep_daily,
        keep_weekly: payload.keep_weekly,
        id: s.id,
        server_id: s.server_id,
        name: s.name,
        cron: s.cron,
        timezone: s.timezone,
        enabled: s.enabled,
        next_run_ms: s.next_run_at,
        last_run_ms: s.last_run_at,
        last_result: s.last_result.unwrap_or_default(),
        ..Default::default()
    }
}

#[allow(refining_impl_trait)]
impl pb::ScheduleService for ScheduleRpc {
    async fn list_schedules(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::ListSchedulesRequest>,
    ) -> ServiceResult<pb::ListSchedulesResponse> {
        require(&ctx, Permission::View)?;
        let schedules = self
            .state
            .auth
            .store()
            .list_schedules(request.server_id)
            .await
            .map_err(store_error)?;
        let names = self.names().await?;
        Response::ok(pb::ListSchedulesResponse {
            schedules: schedules
                .into_iter()
                .map(|s| proto_schedule(s, &names))
                .collect(),
            ..Default::default()
        })
    }

    async fn upsert_schedule(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::UpsertScheduleRequest>,
    ) -> ServiceResult<pb::UpsertScheduleResponse> {
        let user = require(&ctx, Permission::ManageServers)?.clone();
        let s = request
            .schedule
            .as_option()
            .ok_or_else(|| ConnectError::invalid_argument("schedule is required"))?;
        let name = s.name.trim();
        if name.is_empty() || name.chars().count() > 60 {
            return Err(ConnectError::invalid_argument(
                "give the schedule a name (up to 60 characters)",
            ));
        }
        let action = action_name(s.action.as_known())
            .ok_or_else(|| ConnectError::invalid_argument("choose what the schedule does"))?;
        let cron = parse_cron(s.cron).map_err(ConnectError::invalid_argument)?;
        let tz = parse_tz(s.timezone).map_err(ConnectError::invalid_argument)?;
        let text: String = s.text.trim().chars().filter(|c| !c.is_control()).collect();
        if matches!(action, "command" | "broadcast") && text.is_empty() {
            return Err(ConnectError::invalid_argument(if action == "command" {
                "enter the command to run"
            } else {
                "enter the message to send"
            }));
        }
        if text.chars().count() > 500 {
            return Err(ConnectError::invalid_argument(
                "keep it under 500 characters",
            ));
        }
        let mut warn: Vec<u32> = s
            .warn_minutes
            .iter()
            .copied()
            .filter(|m| (1..=60).contains(m))
            .collect();
        warn.sort_unstable_by(|a, b| b.cmp(a));
        warn.dedup();
        let payload = Payload {
            text,
            warn_minutes: if matches!(action, "restart" | "stop") {
                warn
            } else {
                Vec::new()
            },
            keep_daily: if action == "backup" {
                s.keep_daily.min(365)
            } else {
                0
            },
            keep_weekly: if action == "backup" {
                s.keep_weekly.min(520)
            } else {
                0
            },
        };
        let store = self.state.auth.store();
        store
            .find_server(s.server_id)
            .await
            .map_err(store_error)?
            .ok_or_else(|| ConnectError::not_found("server not found"))?;
        let payload_json =
            serde_json::to_string(&payload).map_err(|e| ConnectError::internal(e.to_string()))?;
        let next = if s.enabled {
            crate::schedules::next_run(&cron, &tz, nn_store::now_ms())
        } else {
            None
        };
        let input = ScheduleInput {
            server_id: s.server_id,
            name,
            cron: s.cron.trim(),
            timezone: s.timezone.trim(),
            action,
            payload_json: &payload_json,
            enabled: s.enabled,
            next_run_at: next,
        };
        let saved = if s.id.is_empty() {
            store.insert_schedule(input).await.map_err(store_error)?
        } else {
            let existing = self.load(s.id).await?;
            if existing.server_id != s.server_id {
                return Err(ConnectError::invalid_argument(
                    "a schedule can't move to another server",
                ));
            }
            store
                .update_schedule(s.id, input)
                .await
                .map_err(store_error)?;
            self.load(s.id).await?
        };
        audit(
            &self.state,
            &user,
            if s.id.is_empty() { "schedule.create" } else { "schedule.update" },
            ("server", &saved.server_id),
            json!({ "schedule": saved.name, "action": saved.action, "cron": saved.cron, "enabled": saved.enabled }),
        )
        .await;
        let names = self.names().await?;
        Response::ok(pb::UpsertScheduleResponse {
            schedule: proto_schedule(saved, &names).into(),
            ..Default::default()
        })
    }

    async fn delete_schedule(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::DeleteScheduleRequest>,
    ) -> ServiceResult<pb::DeleteScheduleResponse> {
        let user = require(&ctx, Permission::ManageServers)?.clone();
        let schedule = self.load(request.id).await?;
        self.state
            .auth
            .store()
            .delete_schedule(&schedule.id)
            .await
            .map_err(store_error)?;
        audit(
            &self.state,
            &user,
            "schedule.delete",
            ("server", &schedule.server_id),
            json!({ "schedule": schedule.name }),
        )
        .await;
        Response::ok(pb::DeleteScheduleResponse::default())
    }

    async fn run_schedule_now(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::RunScheduleNowRequest>,
    ) -> ServiceResult<pb::RunScheduleNowResponse> {
        let user = require(&ctx, Permission::ManageServers)?.clone();
        let schedule = self.load(request.id).await?;
        let payload: Payload = serde_json::from_str(&schedule.payload_json).unwrap_or_default();
        audit(
            &self.state,
            &user,
            "schedule.run_now",
            ("server", &schedule.server_id),
            json!({ "schedule": schedule.name }),
        )
        .await;
        let state = self.state.clone();
        tokio::spawn(async move {
            let now = nn_store::now_ms();
            let text = match runner::run(&state, &schedule, &payload).await {
                Ok(m) => m,
                Err(m) => format!("Failed: {m}"),
            };
            let _ = state
                .auth
                .store()
                .record_schedule_run(&schedule.id, now, &text, schedule.next_run_at)
                .await;
        });
        Response::ok(pb::RunScheduleNowResponse::default())
    }

    async fn preview_schedule(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::PreviewScheduleRequest>,
    ) -> ServiceResult<pb::PreviewScheduleResponse> {
        require(&ctx, Permission::View)?;
        let parsed = parse_cron(request.cron).and_then(|c| Ok((c, parse_tz(request.timezone)?)));
        Response::ok(match parsed {
            Ok((cron, tz)) => pb::PreviewScheduleResponse {
                next_runs_ms: next_runs(&cron, &tz, nn_store::now_ms(), 5),
                description: cron.describe(),
                ..Default::default()
            },
            Err(error) => pb::PreviewScheduleResponse {
                error,
                ..Default::default()
            },
        })
    }
}
