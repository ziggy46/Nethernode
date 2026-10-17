//! Servers' resource use over time, kept by the panel (see
//! `nn_store::stats`). Every `server_stats` event, from this machine or
//! another, is added to its server's current minute; finished minutes are
//! written to that minute's and that hour's summaries. Minutes are kept for
//! a few days and hours for about a month.

use std::collections::HashMap;
use std::time::Duration;

use nn_events::Event;
use nn_store::{StatPeriod, now_ms};
use tokio::sync::broadcast::error::RecvError;

use crate::AppState;

const MINUTE_MS: i64 = 60_000;
const HOUR_MS: i64 = 3_600_000;
const DAY_MS: i64 = 24 * HOUR_MS;
/// How long per-minute summaries are kept.
pub(crate) const KEEP_MINUTES_MS: i64 = 3 * DAY_MS;
/// How long hourly summaries are kept.
pub(crate) const KEEP_HOURS_MS: i64 = 35 * DAY_MS;

/// One sample, from a `server_stats` event.
#[derive(Debug, Clone, Copy, Default)]
struct Sample {
    cpu: f64,
    memory: u64,
    limit: u64,
    /// Cumulative since the container started.
    counters: Counters,
    players: Option<u32>,
    mspt: Option<f64>,
    tps: Option<f64>,
    paused: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Counters {
    rx: u64,
    tx: u64,
    read: u64,
    write: u64,
}

/// A server's minute in progress.
#[derive(Debug, Default)]
struct Open {
    minute: StatPeriod,
    /// The previous sample's counters, to turn them into bytes per period.
    last: Option<Counters>,
}

/// How much a counter grew. A smaller value means the container restarted
/// and the counter started again from zero.
fn grew(now: u64, before: u64) -> i64 {
    i64::try_from(if now >= before { now - before } else { now }).unwrap_or(i64::MAX)
}

impl Open {
    /// Adds a sample taken at `now`. Returns the previous minute if this
    /// sample starts a new one.
    fn add(&mut self, now: i64, s: &Sample) -> Option<StatPeriod> {
        let start = now - now.rem_euclid(MINUTE_MS);
        let done = (self.minute.samples > 0 && self.minute.at_ms != start)
            .then(|| std::mem::take(&mut self.minute));
        let m = &mut self.minute;
        if m.samples == 0 {
            m.at_ms = start;
        }
        let c = s.counters;
        if let Some(last) = self.last {
            m.net_rx += grew(c.rx, last.rx);
            m.net_tx += grew(c.tx, last.tx);
            m.disk_read += grew(c.read, last.read);
            m.disk_write += grew(c.write, last.write);
        }
        self.last = Some(c);
        let memory = i64::try_from(s.memory).unwrap_or(i64::MAX);
        m.cpu_max = if m.samples == 0 {
            s.cpu
        } else {
            m.cpu_max.max(s.cpu)
        };
        m.samples += 1;
        m.cpu_sum += s.cpu;
        m.memory_sum = m.memory_sum.saturating_add(memory);
        m.memory_max = m.memory_max.max(memory);
        m.memory_limit = i64::try_from(s.limit).unwrap_or(i64::MAX);
        if let Some(players) = s.players {
            m.players_max = Some(m.players_max.unwrap_or(0).max(i64::from(players)));
        }
        if let (Some(mspt), Some(tps)) = (s.mspt, s.tps) {
            m.tick_samples += 1;
            m.mspt_sum += mspt;
            m.mspt_max = Some(m.mspt_max.map_or(mspt, |x| x.max(mspt)));
            m.tps_sum += tps;
            m.tps_min = Some(m.tps_min.map_or(tps, |x| x.min(tps)));
        }
        m.paused_samples += i64::from(s.paused);
        done
    }

    /// The minute in progress, if it ended before `now`.
    fn finished(&mut self, now: i64) -> Option<StatPeriod> {
        let start = now - now.rem_euclid(MINUTE_MS);
        (self.minute.samples > 0 && self.minute.at_ms < start)
            .then(|| std::mem::take(&mut self.minute))
    }
}

fn sample(event: &Event) -> Option<(&str, Sample)> {
    let Event::ServerStats {
        server_id,
        cpu_percent,
        memory_bytes,
        memory_limit_bytes,
        net_rx_bytes,
        net_tx_bytes,
        disk_read_bytes,
        disk_write_bytes,
        players_online,
        mspt,
        tps,
        paused,
        ..
    } = event
    else {
        return None;
    };
    Some((
        server_id,
        Sample {
            cpu: *cpu_percent,
            memory: *memory_bytes,
            limit: *memory_limit_bytes,
            counters: Counters {
                rx: *net_rx_bytes,
                tx: *net_tx_bytes,
                read: *disk_read_bytes,
                write: *disk_write_bytes,
            },
            players: *players_online,
            mspt: *mspt,
            tps: *tps,
            paused: *paused,
        },
    ))
}

/// Writes a finished minute to its minute and hour.
fn save(state: &AppState, server_id: &str, minute: StatPeriod) {
    let (state, id) = (state.clone(), server_id.to_owned());
    tokio::spawn(async move {
        let store = state.auth.store();
        let hour = StatPeriod {
            at_ms: minute.at_ms - minute.at_ms.rem_euclid(HOUR_MS),
            ..minute.clone()
        };
        let saved = async {
            store.add_stats(&id, 60, &minute).await?;
            store.add_stats(&id, 3600, &hour).await
        };
        // Fails for a server deleted meanwhile.
        if let Err(e) = saved.await {
            tracing::debug!(server = %id, error = %e, "couldn't save a server's stats");
        }
    });
}

/// Records every server's stats as they arrive, for as long as the panel
/// runs.
pub fn spawn_recorder(state: AppState) {
    tokio::spawn(async move {
        let mut events = state.events.subscribe();
        let mut open: HashMap<String, Open> = HashMap::new();
        let mut sweep = tokio::time::interval(Duration::from_secs(20));
        let mut prune = tokio::time::interval(Duration::from_secs(3600));
        loop {
            tokio::select! {
                event = events.recv() => match event {
                    Ok(event) => {
                        if let Some((id, s)) = sample(&event) {
                            let entry = open.entry(id.to_owned()).or_default();
                            if let Some(done) = entry.add(now_ms(), &s) {
                                save(&state, id, done);
                            }
                        }
                    }
                    Err(RecvError::Lagged(_)) => {}
                    Err(RecvError::Closed) => return,
                },
                // Minutes of servers that stopped sending (stopped, or their
                // machine went away).
                _ = sweep.tick() => {
                    let now = now_ms();
                    for (id, entry) in &mut open {
                        if let Some(done) = entry.finished(now) {
                            save(&state, id, done);
                        }
                    }
                }
                _ = prune.tick() => {
                    let store = state.auth.store();
                    let now = now_ms();
                    for (period, keep) in [(60, KEEP_MINUTES_MS), (3600, KEEP_HOURS_MS)] {
                        if let Err(e) = store.prune_stats(period, now - keep).await {
                            tracing::warn!(error = %e, "couldn't remove old server stats");
                        }
                    }
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(cpu: f64, rx: u64, players: Option<u32>, tps: Option<f64>) -> Sample {
        Sample {
            cpu,
            memory: 100,
            limit: 1000,
            counters: Counters {
                rx,
                ..Default::default()
            },
            players,
            mspt: tps.map(|_| 10.0),
            tps,
            paused: false,
        }
    }

    #[test]
    fn samples_add_up_per_minute() {
        let mut open = Open::default();
        let base = 10 * MINUTE_MS;
        assert!(
            open.add(base + 1_000, &at(10.0, 500, None, Some(20.0)))
                .is_none()
        );
        assert!(
            open.add(base + 2_000, &at(30.0, 800, Some(2), Some(18.0)))
                .is_none()
        );
        // The container restarted: the counter starts again from zero.
        assert!(
            open.add(base + 3_000, &at(20.0, 100, Some(1), None))
                .is_none()
        );
        assert!(open.finished(base + 59_000).is_none(), "still going");

        let minute = open.add(base + 61_000, &at(5.0, 150, None, None)).unwrap();
        assert_eq!(minute.at_ms, base);
        assert_eq!(minute.samples, 3);
        assert_eq!(minute.cpu_sum, 60.0);
        assert_eq!(minute.cpu_max, 30.0);
        assert_eq!(
            minute.net_rx,
            300 + 100,
            "the first sample has nothing to compare with"
        );
        assert_eq!(minute.players_max, Some(2));
        assert_eq!(minute.tick_samples, 2);
        assert_eq!(minute.tps_min, Some(18.0));

        let next = open.finished(base + 2 * MINUTE_MS).unwrap();
        assert_eq!(next.at_ms, base + MINUTE_MS);
        assert_eq!(next.net_rx, 50);
        assert!(open.finished(base + 3 * MINUTE_MS).is_none(), "nothing new");
    }
}
