//! Runs schedules when they're due.

use std::collections::HashSet;
use std::time::Duration;

use nn_store::Schedule;
use serde_json::json;

use super::{Payload, next_run, parse_cron, parse_tz, warning};
use crate::AppState;
use crate::backups::{self, Kind};

/// How often the runner checks for due schedules.
const TICK: Duration = Duration::from_secs(5);
/// A run this late is "missed" (NetherNode was down), not just a slow tick.
const LATE: i64 = 2 * 60 * 1000;
/// How late a countdown warning can still be shown.
const WARN_WINDOW: i64 = 2 * TICK.as_millis() as i64;

/// Starts the runner.
pub fn spawn(state: AppState) {
    tokio::spawn(async move {
        // Warnings sent, by (schedule, run time, minutes before).
        let mut warned: HashSet<(String, i64, u32)> = HashSet::new();
        let mut tick = tokio::time::interval(TICK);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tick.tick().await;
            let now = nn_store::now_ms();
            let Ok(schedules) = state.auth.store().list_schedules(None).await else {
                continue;
            };
            for schedule in schedules.into_iter().filter(|s| s.enabled) {
                check(&state, schedule, now, &mut warned).await;
            }
            warned.retain(|(_, at, _)| *at > now - 3_600_000);
        }
    });
}

/// When a schedule next runs after `after_ms`, if it can.
pub(crate) fn upcoming(schedule: &Schedule, after_ms: i64) -> Option<i64> {
    let cron = parse_cron(&schedule.cron).ok()?;
    let tz = parse_tz(&schedule.timezone).ok()?;
    next_run(&cron, &tz, after_ms)
}

async fn check(
    state: &AppState,
    schedule: Schedule,
    now: i64,
    warned: &mut HashSet<(String, i64, u32)>,
) {
    let store = state.auth.store();
    let Some(next) = schedule.next_run_at else {
        // New, or its expression was fixed: work out the next run.
        let _ = store
            .set_schedule_next(&schedule.id, upcoming(&schedule, now))
            .await;
        return;
    };
    let payload: Payload = serde_json::from_str(&schedule.payload_json).unwrap_or_default();

    if now < next {
        if matches!(schedule.action.as_str(), "restart" | "stop") {
            for &minutes in &payload.warn_minutes {
                let at = next - i64::from(minutes) * 60_000;
                // A warning whose moment has long passed (the schedule was just
                // made, or NetherNode was down) is marked sent but not shown,
                // so players never get a burst of stale countdowns.
                if now >= at
                    && warned.insert((schedule.id.clone(), next, minutes))
                    && now - at < WARN_WINDOW
                {
                    let _ = broadcast(
                        state,
                        &schedule.server_id,
                        &warning(&schedule.action, minutes),
                    )
                    .await;
                }
            }
        }
        return;
    }

    // Due. Work out the following run first, so a slow action can't run twice.
    let following = upcoming(&schedule, now);
    let missed = now - next > LATE;
    // Missed messages are stale, and a missed restart or stop would kick
    // players without its countdown; backups and starts still catch up.
    if missed
        && matches!(
            schedule.action.as_str(),
            "command" | "broadcast" | "restart" | "stop"
        )
    {
        let _ = store
            .record_schedule_run(
                &schedule.id,
                now,
                "Skipped: NetherNode wasn't running at the scheduled time.",
                following,
            )
            .await;
        return;
    }
    let _ = store
        .record_schedule_run(&schedule.id, now, "Running…", following)
        .await;
    let state = state.clone();
    tokio::spawn(async move {
        let result = run(&state, &schedule, &payload).await;
        let text = match &result {
            Ok(m) => m.clone(),
            Err(m) => format!("Failed: {m}"),
        };
        let _ = state
            .auth
            .store()
            .record_schedule_run(&schedule.id, now, &text, following)
            .await;
        let entry = nn_store::NewAuditEntry {
            user_id: None,
            action: "schedule.run",
            target_type: Some("server"),
            target_id: Some(&schedule.server_id),
            details: Some(
                json!({ "schedule": schedule.name, "action": schedule.action, "result": text, "missed": missed }),
            ),
        };
        let _ = state.auth.store().insert_audit(entry).await;
        tracing::info!(schedule = %schedule.name, server = %schedule.server_id, result = %text, "schedule ran");
    });
}

/// Runs a schedule's action now. Returns what happened.
pub(crate) async fn run(
    state: &AppState,
    schedule: &Schedule,
    payload: &Payload,
) -> Result<String, String> {
    let store = state.auth.store();
    let server = store
        .find_server(&schedule.server_id)
        .await
        .map_err(|e| e.to_string())?
        .ok_or("the server no longer exists")?;
    let node = state.nodes.for_server(&server).clone();
    let running = server.status == "running";
    match schedule.action.as_str() {
        "backup" => {
            let message = backups::backup_and_wait(state, &server, Kind::Scheduled).await?;
            let tz = parse_tz(&schedule.timezone).unwrap_or(jiff::tz::TimeZone::UTC);
            let pruned = if payload.keep_daily > 0 || payload.keep_weekly > 0 {
                backups::prune(
                    state,
                    &server.id,
                    payload.keep_daily,
                    payload.keep_weekly,
                    &tz,
                )
                .await
            } else {
                0
            };
            Ok(if pruned > 0 {
                format!("{message} Removed {pruned} older backups.")
            } else {
                message
            })
        }
        "restart" | "stop" | "start" | "command" | "broadcast" => {
            if node.unavailable().is_some() {
                return Err("Docker isn't available".into());
            }
            match schedule.action.as_str() {
                "restart" if running => {
                    let _ = broadcast(state, &server.id, "The server is restarting now.").await;
                    node.restart(&server.id).await.map_err(|e| e.to_string())?;
                    Ok("Restarted.".into())
                }
                "stop" if running => {
                    let _ = broadcast(state, &server.id, "The server is stopping now.").await;
                    node.stop(&server.id, false)
                        .await
                        .map_err(|e| e.to_string())?;
                    Ok("Stopped.".into())
                }
                "start" if !running => {
                    node.start(&server.id).await.map_err(|e| e.to_string())?;
                    Ok("Started.".into())
                }
                "command" if running => {
                    let reply = node
                        .command(&server.id, &payload.text)
                        .await
                        .map_err(|e| e.to_string())?;
                    let reply = reply.trim();
                    Ok(if reply.is_empty() {
                        "Sent.".into()
                    } else {
                        format!("Sent. Reply: {}", truncate(reply, 200))
                    })
                }
                "broadcast" if running => {
                    broadcast(state, &server.id, &payload.text).await?;
                    Ok("Sent.".into())
                }
                "start" => Ok("Skipped: already running.".into()),
                _ => Ok(format!("Skipped: {} isn't running.", server.name)),
            }
        }
        other => Err(format!("unknown action {other}")),
    }
}

/// Shows a message to everyone on a running server.
pub(crate) async fn broadcast(state: &AppState, server_id: &str, text: &str) -> Result<(), String> {
    let node = state.nodes.for_server_id(server_id).await;
    if node.unavailable().is_some() {
        return Err("Docker isn't available".into());
    }
    // Bedrock's tellraw takes another format; `say` works everywhere.
    let bedrock = state
        .auth
        .store()
        .find_server(server_id)
        .await
        .ok()
        .flatten()
        .is_some_and(|server| nn_orchestrator::spec::is_bedrock(&server.loader));
    if bedrock {
        node.command(server_id, &format!("say {text}"))
            .await
            .map_err(|e| e.to_string())?;
        return Ok(());
    }
    let component = json!(["", { "text": "[Server] ", "color": "gold" }, { "text": text }]);
    let reply = node
        .command(server_id, &format!("tellraw @a {component}"))
        .await
        .map_err(|e| e.to_string())?;
    // Very old servers don't know tellraw.
    if reply.to_ascii_lowercase().contains("unknown") {
        node.command(server_id, &format!("say {text}"))
            .await
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_owned()
    } else {
        format!("{}…", s.chars().take(max).collect::<String>())
    }
}
