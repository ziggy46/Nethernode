//! Restarts, stops and updates that warn players first: a countdown in
//! chat, then the action. One countdown per server at a time; it can be
//! called off until it runs. Scheduled restarts and stops have their own
//! warnings (see [`crate::schedules`]).

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use nn_events::Event;

use crate::AppState;
use crate::schedules::runner::broadcast;

/// The longest a countdown can be.
pub(crate) const MAX_MINUTES: u32 = 30;
/// Minutes before the action that get a warning (plus the start).
const MARKS: [u32; 4] = [15, 10, 5, 1];

/// What a countdown ends in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Action {
    Restart,
    Stop,
    /// A modpack update (the server restarts on the new version).
    Update,
}

impl Action {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Restart => "restart",
            Self::Stop => "stop",
            Self::Update => "update",
        }
    }

    fn warning(self, left: Duration) -> String {
        let secs = left.as_secs();
        let when = match secs {
            0..=59 => format!("{secs} seconds"),
            60..=119 => "1 minute".to_owned(),
            _ => format!("{} minutes", secs.div_ceil(60)),
        };
        match self {
            Self::Restart => format!("The server is restarting in {when}."),
            Self::Stop => format!("The server is stopping in {when}."),
            Self::Update => format!("The server is restarting for an update in {when}."),
        }
    }

    fn called_off(self) -> &'static str {
        match self {
            Self::Restart => "The restart was called off.",
            Self::Stop => "The stop was called off.",
            Self::Update => "The update was called off.",
        }
    }
}

struct Pending {
    action: Action,
    at_ms: i64,
    id: u64,
    /// Replaced by the same thing done now: ends without telling players
    /// it was called off.
    superseded: bool,
}

static PENDING: LazyLock<Mutex<HashMap<String, Pending>>> = LazyLock::new(Default::default);
static NEXT: AtomicU64 = AtomicU64::new(1);

fn pending_map() -> std::sync::MutexGuard<'static, HashMap<String, Pending>> {
    PENDING.lock().unwrap_or_else(|e| e.into_inner())
}

/// The countdown running for a server: its action and when it ends (unix
/// ms).
pub(crate) fn pending(server_id: &str) -> Option<(Action, i64)> {
    pending_map()
        .get(server_id)
        .filter(|p| !p.superseded)
        .map(|p| (p.action, p.at_ms))
}

/// A countdown that's been set up; [`Ticket::wait`] runs it.
pub(crate) struct Ticket {
    server_id: String,
    action: Action,
    id: u64,
    end: tokio::time::Instant,
}

/// Starts counting down `minutes` (1 to [`MAX_MINUTES`]) for a server.
/// Fails when one is already running.
pub(crate) fn begin(
    state: &AppState,
    server_id: &str,
    action: Action,
    minutes: u32,
) -> Result<Ticket, String> {
    if !(1..=MAX_MINUTES).contains(&minutes) {
        return Err(format!("A countdown is 1 to {MAX_MINUTES} minutes."));
    }
    let length = Duration::from_secs(u64::from(minutes) * 60);
    let id = NEXT.fetch_add(1, Ordering::Relaxed);
    {
        let mut map = pending_map();
        if let Some(p) = map.get(server_id) {
            return Err(format!(
                "A {} is already counting down; call it off first.",
                p.action.as_str()
            ));
        }
        map.insert(
            server_id.to_owned(),
            Pending {
                action,
                at_ms: nn_store::now_ms() + i64::try_from(length.as_millis()).unwrap_or(0),
                id,
                superseded: false,
            },
        );
    }
    changed(state, server_id);
    Ok(Ticket {
        server_id: server_id.to_owned(),
        action,
        id,
        end: tokio::time::Instant::now() + length,
    })
}

/// Calls off a server's countdown. Returns whether there was one.
pub(crate) fn cancel(state: &AppState, server_id: &str) -> bool {
    let removed = pending_map().remove(server_id).is_some();
    if removed {
        changed(state, server_id);
    }
    removed
}

/// Ends a server's countdown quietly, because the server is being
/// restarted, stopped or killed right now instead.
pub(crate) fn supersede(state: &AppState, server_id: &str) {
    let found = match pending_map().get_mut(server_id) {
        Some(p) => {
            p.superseded = true;
            true
        }
        None => false,
    };
    if found {
        changed(state, server_id);
    }
}

fn changed(state: &AppState, server_id: &str) {
    state.events.publish(Event::ServerCountdownChanged {
        server_id: server_id.to_owned(),
    });
}

/// Where a countdown stands.
enum Standing {
    On,
    /// Called off: players are told.
    CalledOff,
    /// Replaced by the same thing done now: players aren't told.
    Replaced,
}

impl Ticket {
    fn standing(&self) -> Standing {
        let mut map = pending_map();
        match map.get(&self.server_id) {
            Some(p) if p.id == self.id && !p.superseded => Standing::On,
            Some(p) if p.id == self.id => {
                map.remove(&self.server_id);
                Standing::Replaced
            }
            _ => Standing::CalledOff,
        }
    }

    /// Warns players on the way down. True when it ran to the end; false
    /// when it was called off or replaced.
    pub(crate) async fn wait(self, state: &AppState) -> bool {
        let left = self.end - tokio::time::Instant::now();
        let _ = broadcast(state, &self.server_id, &self.action.warning(left)).await;
        let mut stops: Vec<Duration> = MARKS
            .iter()
            .map(|m| Duration::from_secs(u64::from(*m) * 60))
            .chain([Duration::from_secs(10)])
            .filter(|d| *d < left)
            .collect();
        stops.sort_unstable_by(|a, b| b.cmp(a));
        for before in stops.into_iter().chain([Duration::ZERO]) {
            loop {
                match self.standing() {
                    Standing::On => {}
                    Standing::CalledOff => {
                        let _ = broadcast(state, &self.server_id, self.action.called_off()).await;
                        return false;
                    }
                    Standing::Replaced => return false,
                }
                let now = tokio::time::Instant::now();
                if now + before >= self.end {
                    break;
                }
                // Wake each second so a cancel is noticed quickly.
                tokio::time::sleep_until((self.end - before).min(now + Duration::from_secs(1)))
                    .await;
            }
            if !before.is_zero() {
                let _ = broadcast(state, &self.server_id, &self.action.warning(before)).await;
            }
        }
        // Done: no longer pending, whatever happens next.
        let ours = {
            let mut map = pending_map();
            let ours = map.get(&self.server_id).is_some_and(|p| p.id == self.id);
            if ours {
                map.remove(&self.server_id);
            }
            ours
        };
        changed(state, &self.server_id);
        ours
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn warnings_read_naturally() {
        let w = |action: Action, secs| action.warning(Duration::from_secs(secs));
        assert_eq!(
            w(Action::Restart, 300),
            "The server is restarting in 5 minutes."
        );
        assert_eq!(w(Action::Stop, 60), "The server is stopping in 1 minute.");
        assert_eq!(
            w(Action::Update, 10),
            "The server is restarting for an update in 10 seconds."
        );
        // A countdown that's just begun rounds up.
        assert_eq!(
            w(Action::Restart, 299),
            "The server is restarting in 5 minutes."
        );
    }
}
