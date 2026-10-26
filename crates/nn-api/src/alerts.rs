//! Resource alerts for the Discord webhook: a machine running low on disk, a
//! server that stays below full speed, a server with its memory nearly full.
//!
//! Each is said once when it starts, then not again until things have been
//! fine for a while, so a server hovering around the line doesn't send a
//! message every minute. Servers and machines are measured the way the bell
//! in the top bar and the Performance tab measure them.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use nn_events::Event;
use serde_json::json;
use tokio::sync::broadcast::error::RecvError;

use crate::AppState;
use crate::notices::{gb, low};
use crate::notify::COLOR_BAD;

/// How often machines' disks are looked at.
const DISK_EVERY: Duration = Duration::from_secs(60);
/// Below this many ticks per second a server is struggling (20 is full speed).
const LAG_BELOW: f64 = 15.0;
/// Back above this, it's fine again.
const LAG_FINE: f64 = 18.0;
/// How long it has to struggle before anyone is told.
const LAG_FOR: Duration = Duration::from_secs(5 * 60);
/// Memory use, as a share of the server's limit, that counts as nearly full.
const MEMORY_FULL: f64 = 0.95;
/// Back below this, it's fine again.
const MEMORY_FINE: f64 = 0.85;
/// How long it has to stay full before anyone is told (a Java server fills
/// up and tidies itself all the time).
const MEMORY_FOR: Duration = Duration::from_secs(10 * 60);
/// How long things have to stay fine before the next problem is told afresh.
const CALM_FOR: Duration = Duration::from_secs(5 * 60);

/// One thing being watched.
#[derive(Debug, Default)]
pub(crate) struct Watch {
    bad_since: Option<Instant>,
    fine_since: Option<Instant>,
    raised: bool,
}

impl Watch {
    /// Takes a measurement. `bad` is past the line, `fine` is clearly back
    /// from it (between the two changes nothing). Returns true once, when
    /// it has been bad for `hold` and nobody has been told yet.
    pub(crate) fn observe(&mut self, now: Instant, bad: bool, fine: bool, hold: Duration) -> bool {
        if bad {
            self.fine_since = None;
            let since = *self.bad_since.get_or_insert(now);
            if !self.raised && now.duration_since(since) >= hold {
                self.raised = true;
                return true;
            }
            return false;
        }
        if !self.raised {
            self.bad_since = None;
            return false;
        }
        if fine {
            let since = *self.fine_since.get_or_insert(now);
            if now.duration_since(since) >= CALM_FOR {
                *self = Watch::default();
            }
        } else {
            self.fine_since = None;
        }
        false
    }
}

/// What's watched for one server.
#[derive(Debug, Default)]
struct ServerWatch {
    lag: Watch,
    memory: Watch,
}

/// Starts watching, for as long as the panel runs.
pub fn spawn(state: AppState) {
    tokio::spawn(async move {
        let mut events = state.events.subscribe();
        let mut servers: HashMap<String, ServerWatch> = HashMap::new();
        let mut disks: HashMap<String, Watch> = HashMap::new();
        let start = tokio::time::Instant::now() + DISK_EVERY;
        let mut disk_tick = tokio::time::interval_at(start, DISK_EVERY);
        loop {
            tokio::select! {
                event = events.recv() => match event {
                    Ok(Event::ServerStats {
                        server_id,
                        memory_bytes,
                        memory_limit_bytes,
                        tps,
                        ..
                    }) => {
                        let now = Instant::now();
                        let watch = servers.entry(server_id.clone()).or_default();
                        // A server paused for want of players reports no TPS.
                        if let Some(tps) = tps
                            && watch.lag.observe(now, tps < LAG_BELOW, tps >= LAG_FINE, LAG_FOR)
                        {
                            say(&state, lag_embed(&server_name(&state, &server_id).await, tps)).await;
                        }
                        if memory_limit_bytes > 0 {
                            let used = memory_bytes as f64 / memory_limit_bytes as f64;
                            if watch.memory.observe(
                                now,
                                used >= MEMORY_FULL,
                                used < MEMORY_FINE,
                                MEMORY_FOR,
                            ) {
                                let name = server_name(&state, &server_id).await;
                                say(&state, memory_embed(&name, memory_limit_bytes)).await;
                            }
                        }
                    }
                    // A server that stopped starts over when it's back.
                    Ok(Event::ServerStatusChanged { server_id, status })
                        if matches!(status.as_str(), "stopped" | "crashed" | "deleted") =>
                    {
                        servers.remove(&server_id);
                    }
                    Ok(_) | Err(RecvError::Lagged(_)) => {}
                    Err(RecvError::Closed) => return,
                },
                _ = disk_tick.tick() => check_disks(&state, &mut disks).await,
            }
        }
    });
}

/// A machine's ID, its name, and its disk's total and free bytes when known.
type Machine = (String, String, Option<(u64, u64)>);

/// Looks at every machine's disk.
async fn check_disks(state: &AppState, watches: &mut HashMap<String, Watch>) {
    let mut machines: Vec<Machine> = vec![(
        "local".to_owned(),
        "The panel's machine".to_owned(),
        state.nodes.local().disk_space(),
    )];
    for node in state.auth.store().list_nodes().await.unwrap_or_default() {
        if node.is_local() {
            continue;
        }
        let space = state
            .agents
            .live(&node.id)
            .and_then(|l| l.host)
            .and_then(|h| Some((h.disk_total_bytes?, h.disk_free_bytes?)));
        machines.push((node.id.clone(), node.name.clone(), space));
    }
    let now = Instant::now();
    for (id, name, space) in machines {
        // A machine that can't be reached has nothing to measure.
        let Some((total, free)) = space else {
            continue;
        };
        let is_low = low(total, free);
        if watches
            .entry(id)
            .or_default()
            .observe(now, is_low, !is_low, Duration::ZERO)
        {
            say(state, disk_embed(&name, total, free)).await;
        }
    }
}

async fn server_name(state: &AppState, id: &str) -> String {
    match state.auth.store().find_server(id).await {
        Ok(Some(server)) => server.name,
        _ => "A server".to_owned(),
    }
}

/// Sends one alert, if resource alerts are on.
async fn say(state: &AppState, embed: serde_json::Value) {
    if !state.notifier.alerts_on() {
        return;
    }
    let mut embed = embed;
    embed["timestamp"] = json!(jiff::Timestamp::now().to_string());
    if let Err(e) = state.notifier.send(vec![embed]).await {
        tracing::warn!(error = %e, "couldn't send a resource alert");
    }
}

fn lag_embed(name: &str, tps: f64) -> serde_json::Value {
    json!({
        "title": format!("🐌 {name} is struggling"),
        "description": format!(
            "It has run below {LAG_BELOW} ticks per second for {} minutes (now {tps:.1}; 20 is full speed). The Performance tab shows what changed. Fewer loaded chunks, fewer mods, or more memory often help.",
            LAG_FOR.as_secs() / 60
        ),
        "color": COLOR_BAD,
    })
}

fn memory_embed(name: &str, limit: u64) -> serde_json::Value {
    json!({
        "title": format!("🧠 {name} is almost out of memory"),
        "description": format!(
            "It has used over {}% of its {} for {} minutes. If it keeps happening, give it more under Configuration before it crashes.",
            (MEMORY_FULL * 100.0).round() as u32,
            gb(limit),
            MEMORY_FOR.as_secs() / 60
        ),
        "color": COLOR_BAD,
    })
}

fn disk_embed(machine: &str, total: u64, free: u64) -> serde_json::Value {
    json!({
        "title": format!("💽 {machine} is running out of disk space"),
        "description": format!(
            "{} free of {}. Backups, uploads and worlds fail once it's full.",
            gb(free),
            gb(total)
        ),
        "color": COLOR_BAD,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(secs: u64, base: Instant) -> Instant {
        base + Duration::from_secs(secs)
    }

    #[test]
    fn says_once_after_the_hold_and_not_again_while_it_lasts() {
        let base = Instant::now();
        let mut w = Watch::default();
        let hold = Duration::from_secs(300);
        assert!(!w.observe(at(0, base), true, false, hold));
        assert!(!w.observe(at(299, base), true, false, hold));
        assert!(w.observe(at(300, base), true, false, hold));
        assert!(!w.observe(at(301, base), true, false, hold));
        assert!(!w.observe(at(3000, base), true, false, hold));
    }

    #[test]
    fn a_blip_resets_the_clock() {
        let base = Instant::now();
        let mut w = Watch::default();
        let hold = Duration::from_secs(300);
        assert!(!w.observe(at(0, base), true, false, hold));
        assert!(!w.observe(at(200, base), false, true, hold));
        // The count starts over.
        assert!(!w.observe(at(210, base), true, false, hold));
        assert!(!w.observe(at(400, base), true, false, hold));
        assert!(w.observe(at(510, base), true, false, hold));
    }

    #[test]
    fn it_is_told_again_only_after_a_calm_stretch() {
        let base = Instant::now();
        let mut w = Watch::default();
        assert!(w.observe(at(0, base), true, false, Duration::ZERO));
        // Fine for a moment, then bad again: still the same problem.
        assert!(!w.observe(at(10, base), false, true, Duration::ZERO));
        assert!(!w.observe(at(20, base), true, false, Duration::ZERO));
        // Fine for the whole calm stretch.
        assert!(!w.observe(at(30, base), false, true, Duration::ZERO));
        assert!(!w.observe(
            at(30 + CALM_FOR.as_secs(), base),
            false,
            true,
            Duration::ZERO
        ));
        assert!(w.observe(
            at(400 + CALM_FOR.as_secs(), base),
            true,
            false,
            Duration::ZERO
        ));
    }

    #[test]
    fn between_the_lines_changes_nothing() {
        let base = Instant::now();
        let mut w = Watch::default();
        assert!(w.observe(at(0, base), true, false, Duration::ZERO));
        assert!(!w.observe(at(10, base), false, true, Duration::ZERO));
        // Not bad, not clearly fine: the calm count stops.
        assert!(!w.observe(at(20, base), false, false, Duration::ZERO));
        assert!(!w.observe(
            at(10 + CALM_FOR.as_secs(), base),
            false,
            false,
            Duration::ZERO
        ));
        assert!(!w.observe(
            at(11 + CALM_FOR.as_secs(), base),
            true,
            false,
            Duration::ZERO
        ));
    }

    #[test]
    fn messages_say_what_and_how_much() {
        let lag = lag_embed("SMP", 11.24);
        assert_eq!(lag["title"], "🐌 SMP is struggling");
        let text = lag["description"].as_str().unwrap();
        assert!(text.contains("15"), "{text}");
        assert!(text.contains("11.2"), "{text}");
        let memory = memory_embed("SMP", 4 * 1024 * 1024 * 1024);
        let text = memory["description"].as_str().unwrap();
        assert!(text.contains("95%") && text.contains("4.0 GB"), "{text}");
        let disk = disk_embed("nn-node2", 100 << 30, 4 << 30);
        assert_eq!(disk["title"], "💽 nn-node2 is running out of disk space");
    }
}
