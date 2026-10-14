//! Scheduled actions: restarts with countdown warnings, backups with
//! retention, console commands, and chat broadcasts.
//!
//! A runner checks every few seconds whether anything is due (rather than
//! sleeping until the next run), so edits, restarts, and clock changes are
//! picked up without bookkeeping. A run missed while NetherNode was down
//! happens once when it's back, except commands and broadcasts, which are
//! skipped: they're only meaningful at the time.

pub mod runner;

use std::str::FromStr;

use croner::Cron;
use jiff::tz::TimeZone;
use serde::{Deserialize, Serialize};

/// Details of a schedule's action, stored as JSON.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Payload {
    /// The command (command action) or message (broadcast action).
    pub text: String,
    /// Minutes before a restart or stop to warn players.
    pub warn_minutes: Vec<u32>,
    /// Scheduled backups to keep: one per day for this many days...
    pub keep_daily: u32,
    /// ...and one per week for this many weeks.
    pub keep_weekly: u32,
}

/// Actions a schedule can take.
pub const ACTIONS: &[&str] = &["restart", "backup", "command", "broadcast", "start", "stop"];

/// Parses a cron expression (5 fields, or 6 with seconds first).
pub fn parse_cron(expr: &str) -> Result<Cron, String> {
    let expr = expr.trim();
    if expr.is_empty() {
        return Err("enter a cron expression".into());
    }
    let cron = Cron::from_str(expr).map_err(|e| format!("not a valid cron expression: {e}"))?;
    Ok(cron)
}

/// Looks up an IANA time zone.
pub fn parse_tz(name: &str) -> Result<TimeZone, String> {
    TimeZone::get(name.trim()).map_err(|_| format!("unknown time zone {name}"))
}

/// The first run strictly after `after_ms`, in unix ms.
pub fn next_run(cron: &Cron, tz: &TimeZone, after_ms: i64) -> Option<i64> {
    let after = jiff::Timestamp::from_millisecond(after_ms)
        .ok()?
        .to_zoned(tz.clone());
    let next = cron.find_next_occurrence(&after, false).ok()?;
    Some(next.timestamp().as_millisecond())
}

/// The next `n` runs after `after_ms`.
pub fn next_runs(cron: &Cron, tz: &TimeZone, after_ms: i64, n: usize) -> Vec<i64> {
    let mut out = Vec::with_capacity(n);
    let mut at = after_ms;
    while out.len() < n {
        let Some(next) = next_run(cron, tz, at) else {
            break;
        };
        out.push(next);
        at = next;
    }
    out
}

/// The countdown message for `minutes` before an action.
pub fn warning(action: &str, minutes: u32) -> String {
    let what = if action == "stop" {
        "stopping"
    } else {
        "restarting"
    };
    let when = if minutes == 1 {
        "1 minute".to_owned()
    } else {
        format!("{minutes} minutes")
    };
    format!("The server is {what} in {when}.")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(zone: &str, y: i16, mo: i8, d: i8, h: i8, mi: i8) -> i64 {
        jiff::civil::date(y, mo, d)
            .at(h, mi, 0, 0)
            .to_zoned(TimeZone::get(zone).unwrap())
            .unwrap()
            .timestamp()
            .as_millisecond()
    }

    #[test]
    fn next_runs_in_the_schedules_time_zone() {
        let cron = parse_cron("0 4 * * *").unwrap();
        let ny = parse_tz("America/New_York").unwrap();
        let after = ms("America/New_York", 2026, 10, 13, 12, 0);
        assert_eq!(
            next_run(&cron, &ny, after),
            Some(ms("America/New_York", 2026, 10, 14, 4, 0))
        );
        let utc = parse_tz("UTC").unwrap();
        assert_ne!(next_run(&cron, &utc, after), next_run(&cron, &ny, after));
    }

    #[test]
    fn daylight_saving_changes() {
        // 4 AM local every day: 25 hours apart across the November change
        // (1 Nov 2026) and 23 across the March one (8 Mar 2026).
        let cron = parse_cron("0 4 * * *").unwrap();
        let ny = parse_tz("America/New_York").unwrap();
        let fall = next_runs(&cron, &ny, ms("America/New_York", 2026, 10, 30, 12, 0), 2);
        assert_eq!(fall[0], ms("America/New_York", 2026, 10, 31, 4, 0));
        assert_eq!(fall[1] - fall[0], 25 * 3_600_000);
        let spring = next_runs(&cron, &ny, ms("America/New_York", 2026, 3, 6, 12, 0), 2);
        assert_eq!(spring[1] - spring[0], 23 * 3_600_000);
    }

    #[test]
    fn seconds_are_optional_and_bad_input_is_explained() {
        assert!(parse_cron("*/30 * * * * *").is_ok());
        assert!(parse_cron("0 */6 * * *").is_ok());
        assert!(parse_cron("").unwrap_err().contains("enter"));
        assert!(parse_cron("not cron").unwrap_err().contains("not a valid"));
        assert!(parse_cron("61 * * * *").is_err());
        assert!(parse_tz("Mars/Olympus").is_err());
    }

    #[test]
    fn warnings_read_naturally() {
        assert_eq!(
            warning("restart", 5),
            "The server is restarting in 5 minutes."
        );
        assert_eq!(warning("stop", 1), "The server is stopping in 1 minute.");
    }
}
