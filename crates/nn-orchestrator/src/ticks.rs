//! Tick health: how long a running server takes per game tick, the main
//! measure of lag. Read over the server's RCON connection every few
//! seconds: vanilla's `tick query` (1.20.3 and later, including Paper,
//! Fabric and Forge), or Paper and Spigot's `tps` and `mspt` on older
//! versions.

use std::sync::Arc;
use std::time::Duration;

use crate::error::OrchestratorError;
use crate::lifecycle::Orchestrator;

/// How often tick times are read.
pub const TICK_POLL: Duration = Duration::from_secs(5);
/// Failed reads, before a server's first success, after which it's taken
/// not to report tick times (until it restarts).
const GIVE_UP_AFTER: u32 = 6;

/// The latest tick timing.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct TickStats {
    /// Average milliseconds per tick (50 or less keeps up with 20 TPS).
    pub mspt: f64,
    /// The slowest 5% of recent ticks took at least this long, when known.
    pub mspt_p95: Option<f64>,
    /// Ticks per second actually achieved (20 is full speed).
    pub tps: f64,
}

/// Which command a server answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Source {
    TickQuery,
    Paper,
}

/// Reads tick times while the server runs, into its runtime state.
pub(crate) async fn poll(orch: Arc<Orchestrator>, id: String) {
    let mut source = None;
    let mut misses = 0;
    loop {
        tokio::time::sleep(TICK_POLL).await;
        let reading = match source {
            Some(Source::TickQuery) => tick_query(&orch, &id).await,
            Some(Source::Paper) => paper(&orch, &id).await,
            None => match tick_query(&orch, &id).await {
                Ok(Some(t)) => {
                    source = Some(Source::TickQuery);
                    Ok(Some(t))
                }
                Ok(None) => match paper(&orch, &id).await {
                    Ok(Some(t)) => {
                        source = Some(Source::Paper);
                        Ok(Some(t))
                    }
                    other => other,
                },
                Err(e) => Err(e),
            },
        };
        match reading {
            Ok(Some(tick)) => {
                misses = 0;
                orch.runtime(&id).state().tick = Some(tick);
            }
            // Not running yet (or any more), or RCON hiccuped.
            Err(_) => {}
            Ok(None) => {
                orch.runtime(&id).state().tick = None;
                misses += 1;
                if source.is_none() && misses >= GIVE_UP_AFTER {
                    tracing::info!(server = %id, "this server doesn't report tick times");
                    return;
                }
            }
        }
    }
}

/// `Err` when the command couldn't be sent (not running, RCON down);
/// `Ok(None)` when the server answered something unusable.
async fn run(orch: &Orchestrator, id: &str, command: &str) -> Result<String, OrchestratorError> {
    orch.command(id, command).await
}

async fn tick_query(orch: &Orchestrator, id: &str) -> Result<Option<TickStats>, OrchestratorError> {
    Ok(parse_tick_query(&run(orch, id, "tick query").await?))
}

async fn paper(orch: &Orchestrator, id: &str) -> Result<Option<TickStats>, OrchestratorError> {
    let Some(tps) = parse_tps(&run(orch, id, "tps").await?) else {
        return Ok(None);
    };
    let mspt = parse_mspt(&run(orch, id, "mspt").await?).unwrap_or(if tps > 0.0 {
        1000.0 / tps
    } else {
        0.0
    });
    Ok(Some(TickStats {
        mspt,
        mspt_p95: None,
        tps,
    }))
}

/// Vanilla `tick query` output, e.g. "The game is running normally /
/// Target tick rate: 20.0 per second. / Average time per tick: 2.4ms
/// (Target: 50.0ms) / Percentiles: P50: 2.1ms P95: 3.8ms P99: 6.0ms …".
pub fn parse_tick_query(output: &str) -> Option<TickStats> {
    let text = strip_codes(output);
    let mspt = number_after(&text, "Average time per tick:")?;
    let target = number_after(&text, "Target tick rate:").unwrap_or(20.0);
    let frozen = text.contains("frozen");
    let tps = if frozen {
        0.0
    } else if mspt > 0.0 {
        (1000.0 / mspt).min(target)
    } else {
        target
    };
    Some(TickStats {
        mspt,
        mspt_p95: number_after(&text, "P95:"),
        tps,
    })
}

/// Paper/Spigot `tps`: "TPS from last 1m, 5m, 15m: 19.98, 20.0, 20.0".
pub fn parse_tps(output: &str) -> Option<f64> {
    number_after(&strip_codes(output), "TPS from last 1m, 5m, 15m:").map(|t| t.min(20.0))
}

/// Paper `mspt`: "Server tick times (avg/min/max) from last 5s, 10s, 1m:
/// ◴ 1.2/0.5/3.4, …"; the 5-second average.
pub fn parse_mspt(output: &str) -> Option<f64> {
    let text = strip_codes(output);
    let after = &text[text.find("1m:")? + 3..];
    let start = after.find(|c: char| c.is_ascii_digit())?;
    number_at(&after[start..])
}

/// Text without Minecraft's `§x` formatting codes.
fn strip_codes(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '§' {
            chars.next();
        } else {
            out.push(c);
        }
    }
    out
}

/// The first number after `label`, skipping spaces and a leading `*`.
fn number_after(text: &str, label: &str) -> Option<f64> {
    let rest = &text[text.find(label)? + label.len()..];
    number_at(rest.trim_start_matches([' ', '*']))
}

fn number_at(s: &str) -> Option<f64> {
    let end = s
        .find(|c: char| !(c.is_ascii_digit() || c == '.' || c == ','))
        .unwrap_or(s.len());
    s[..end]
        .trim_end_matches([',', '.'])
        .replace(',', ".")
        .parse()
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vanilla_tick_query() {
        let out = "The game is running normally\nTarget tick rate: 20.0 per second.\nAverage time per tick: 2.4ms (Target: 50.0ms)\nPercentiles: P50: 2.1ms P95: 3.8ms P99: 6.0ms. Sample: 100";
        let t = parse_tick_query(out).unwrap();
        assert_eq!((t.mspt, t.mspt_p95, t.tps), (2.4, Some(3.8), 20.0));

        let lagging = "The game is running, but can't keep up with the current tick rate\nTarget tick rate: 20.0 per second.\nAverage time per tick: 80.0ms (Target: 50.0ms)";
        let t = parse_tick_query(lagging).unwrap();
        assert_eq!((t.mspt, t.mspt_p95, t.tps), (80.0, None, 12.5));

        let frozen =
            "The game is frozen\nTarget tick rate: 20.0 per second.\nAverage time per tick: 0.5ms";
        assert_eq!(parse_tick_query(frozen).unwrap().tps, 0.0);

        assert_eq!(
            parse_tick_query("Unknown or incomplete command, see below for error"),
            None
        );
    }

    #[test]
    fn paper_tps_and_mspt() {
        assert_eq!(
            parse_tps("§6TPS from last 1m, 5m, 15m: §a*20.0, §a19.97, §a19.99"),
            Some(20.0)
        );
        assert_eq!(
            parse_tps("§6TPS from last 1m, 5m, 15m: §e17.5, §a19.9, §a20.0"),
            Some(17.5)
        );
        let mspt = "§6Server tick times §e(§7avg§e/§7min§e/§7max§e)§6 from last 5s§7,§6 10s§7,§6 1m§e:\n§6◴ §a1.2§7/§a0.5§7/§a3.4§e, §a1.1§7/§a0.4§7/§a3.9";
        assert_eq!(parse_mspt(mspt), Some(1.2));
        assert_eq!(parse_tps("Unknown command. Type \"/help\" for help."), None);
    }
}
