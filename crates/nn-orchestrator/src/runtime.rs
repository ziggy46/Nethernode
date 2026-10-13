//! In-memory state per server: operation lock, stats history, log backlog,
//! player counts, and crash bookkeeping.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::sync::broadcast;
use tokio::task::JoinHandle;

use crate::ring::Ring;

/// One hour of 1-second samples.
pub const STATS_HISTORY: usize = 3600;
/// Log lines kept for late joiners.
pub const LOG_BACKLOG: usize = 5000;
/// Crash restarts allowed per window.
pub const MAX_CRASH_RESTARTS: usize = 3;
/// Window for counting crash restarts.
pub const CRASH_WINDOW: Duration = Duration::from_secs(10 * 60);

/// A resource sample.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StatSample {
    /// Unix ms.
    pub at_ms: i64,
    /// CPU use; 100 = one core.
    pub cpu_percent: f64,
    /// Memory in use, excluding page cache.
    pub memory_bytes: u64,
    /// Container memory limit.
    pub memory_limit_bytes: u64,
    /// Cumulative bytes received.
    pub net_rx_bytes: u64,
    /// Cumulative bytes sent.
    pub net_tx_bytes: u64,
}

/// Player counts from the last successful status ping.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Players {
    /// Online.
    pub online: u32,
    /// Limit.
    pub max: u32,
}

/// Log fan-out with a backlog for late subscribers.
pub struct LogHub {
    tx: broadcast::Sender<Arc<str>>,
    backlog: Mutex<Ring<Arc<str>>>,
}

impl LogHub {
    fn new() -> Self {
        Self {
            tx: broadcast::channel(1024).0,
            backlog: Mutex::new(Ring::new(LOG_BACKLOG)),
        }
    }

    /// Appends a line and sends it to subscribers.
    pub fn push(&self, line: &str) {
        let line: Arc<str> = Arc::from(line);
        self.backlog
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(line.clone());
        let _ = self.tx.send(line);
    }

    /// The backlog, then live lines.
    pub fn subscribe(&self) -> (Vec<Arc<str>>, broadcast::Receiver<Arc<str>>) {
        // Hold the lock so no line falls between snapshot and subscription.
        let backlog = self.backlog.lock().unwrap_or_else(|e| e.into_inner());
        (backlog.snapshot(), self.tx.subscribe())
    }
}

/// Mutable runtime state.
#[derive(Default)]
pub(crate) struct State {
    /// Set when NetherNode stops the container on purpose: the status to
    /// record when it exits (`stopped` or `sleeping`).
    pub expected_exit: Option<&'static str>,
    /// Monitor, stats, and log tasks for the running container.
    pub tasks: Vec<JoinHandle<()>>,
    pub players: Option<Players>,
    pub crash_restarts: VecDeque<Instant>,
    /// NetherNode is starting the container itself, so Docker's `start`
    /// event shouldn't be treated as an outside start.
    pub launching: bool,
}

impl State {
    pub fn stop_tasks(&mut self) {
        for task in self.tasks.drain(..) {
            task.abort();
        }
    }

    pub fn monitored(&self) -> bool {
        self.tasks.iter().any(|t| !t.is_finished())
    }

    /// Records a crash and says whether an automatic restart is allowed.
    pub fn allow_crash_restart(&mut self, now: Instant) -> bool {
        while self
            .crash_restarts
            .front()
            .is_some_and(|t| now.duration_since(*t) > CRASH_WINDOW)
        {
            self.crash_restarts.pop_front();
        }
        if self.crash_restarts.len() >= MAX_CRASH_RESTARTS {
            return false;
        }
        self.crash_restarts.push_back(now);
        true
    }
}

/// Everything held per server.
pub struct Runtime {
    /// Serializes lifecycle operations on this server.
    pub(crate) op: tokio::sync::Mutex<()>,
    pub(crate) state: Mutex<State>,
    pub(crate) stats: Mutex<Ring<StatSample>>,
    /// Console output.
    pub logs: LogHub,
}

impl Runtime {
    pub(crate) fn new() -> Self {
        Self {
            op: tokio::sync::Mutex::new(()),
            state: Mutex::new(State::default()),
            stats: Mutex::new(Ring::new(STATS_HISTORY)),
            logs: LogHub::new(),
        }
    }

    pub(crate) fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl Drop for Runtime {
    fn drop(&mut self) {
        self.state().stop_tasks();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crash_restarts_are_capped_per_window() {
        let mut state = State::default();
        let t0 = Instant::now();
        assert!(state.allow_crash_restart(t0));
        assert!(state.allow_crash_restart(t0 + Duration::from_secs(60)));
        assert!(state.allow_crash_restart(t0 + Duration::from_secs(120)));
        assert!(
            !state.allow_crash_restart(t0 + Duration::from_secs(180)),
            "4th in 10 min"
        );
        // After the window passes, restarts are allowed again.
        assert!(state.allow_crash_restart(t0 + Duration::from_secs(60 * 11)));
    }

    #[test]
    fn log_hub_replays_backlog_then_streams() {
        let hub = LogHub::new();
        hub.push("one");
        hub.push("two");
        let (backlog, mut rx) = hub.subscribe();
        assert_eq!(
            backlog.iter().map(|l| &**l).collect::<Vec<_>>(),
            ["one", "two"]
        );
        hub.push("three");
        assert_eq!(&*rx.try_recv().unwrap(), "three");
    }
}
