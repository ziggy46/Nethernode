//! Per-container background work while a server runs:
//!
//! - **readiness:** ping until the server answers, then mark it `running`
//! - **players:** ping every `player_poll`; sleep the server when it has
//!   been empty long enough and `sleep_when_empty` is on
//! - **stats:** Docker stats → CPU %, memory, network, once per second
//! - **logs:** Docker logs (follow) → the server's log hub
//!
//! Tasks end on their own when the container stops; the event watcher
//! also aborts them when it sees the container die.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bollard::container::LogOutput;
use bollard::models::ContainerStatsResponse;
use bollard::query_parameters::{LogsOptionsBuilder, StatsOptionsBuilder};
use futures_util::StreamExt;
use nn_events::Event;
use nn_store::now_ms;

use crate::lifecycle::Orchestrator;
use crate::runtime::{Players, Runtime, StatSample};

/// PROXY protocol v2 `LOCAL` header, for servers configured to expect PROXY
/// headers (health checks use LOCAL). Spec: haproxy proxy-protocol.txt §2.2.
const PROXY_V2_LOCAL: [u8; 16] = [
    0x0d, 0x0a, 0x0d, 0x0a, 0x00, 0x0d, 0x0a, 0x51, 0x55, 0x49, 0x54, 0x0a, 0x20, 0x00, 0x00, 0x00,
];
const READY_POLL: Duration = Duration::from_secs(2);
const PING_TIMEOUT: Duration = Duration::from_secs(3);

/// Starts monitoring a running container. `adopted` means NetherNode found
/// it already running (e.g. after a restart of NetherNode itself).
pub(crate) fn spawn(
    orch: &Arc<Orchestrator>,
    id: &str,
    container: &str,
    rt: &Arc<Runtime>,
    adopted: bool,
) {
    let mut state = rt.state();
    state.stop_tasks();
    state.launching = false;
    state.tick = None;
    state.tasks.push(tokio::spawn(crate::ticks::poll(
        orch.clone(),
        id.to_owned(),
    )));
    state.tasks.push(tokio::spawn(watch_health(
        orch.clone(),
        id.to_owned(),
        adopted,
    )));
    state.tasks.push(tokio::spawn(stream_stats(
        orch.clone(),
        id.to_owned(),
        container.to_owned(),
    )));
    state.tasks.push(tokio::spawn(stream_logs(
        orch.clone(),
        rt.clone(),
        container.to_owned(),
        adopted,
    )));
}

/// Pings the server, with and without a PROXY header (whichever it expects).
async fn ping(addr: SocketAddr) -> Option<Players> {
    for prelude in [&[][..], &PROXY_V2_LOCAL[..]] {
        if let Ok(status) = nn_mcproto::client::ping(addr, prelude, PING_TIMEOUT).await {
            let players = status.players.map(|p| Players {
                online: u32::try_from(p.online.max(0)).unwrap_or(u32::MAX),
                max: u32::try_from(p.max.max(0)).unwrap_or(u32::MAX),
            });
            return Some(players.unwrap_or(Players { online: 0, max: 0 }));
        }
    }
    None
}

async fn watch_health(orch: Arc<Orchestrator>, id: String, adopted: bool) {
    let Ok(server) = orch.load(&id).await else {
        return;
    };
    let Ok(port) = u16::try_from(server.internal_port) else {
        return;
    };
    let addr = SocketAddr::new(orch.backend_ip(), port);

    // Readiness: first install can take minutes (downloading the server jar,
    // running the Forge installer), so there's no deadline; the container
    // exiting ends this task.
    let mut players = loop {
        if let Some(players) = ping(addr).await {
            break players;
        }
        if adopted && server.status == "running" {
            // Adopted but not answering: show that it's (re)starting.
            orch.set_status(&id, "starting").await;
        }
        tokio::time::sleep(READY_POLL).await;
    };
    if orch
        .load(&id)
        .await
        .map(|s| s.status != "running")
        .unwrap_or(false)
    {
        orch.set_status(&id, "running").await;
        let _ = orch.store.mark_server_started(&id, now_ms()).await;
    }
    publish_players(&orch, &id, players);

    let mut empty_since = (players.online == 0).then(Instant::now);
    loop {
        tokio::time::sleep(orch.config.player_poll).await;
        let Some(now) = ping(addr).await else {
            continue;
        };
        if now != players {
            players = now;
            publish_players(&orch, &id, players);
        }
        if players.online > 0 {
            empty_since = None;
            continue;
        }
        let since = *empty_since.get_or_insert_with(Instant::now);
        let Ok(server) = orch.load(&id).await else {
            return;
        };
        if !server.sleep_when_empty || server.status != "running" {
            continue;
        }
        let idle = server
            .auto_stop_idle_minutes
            .unwrap_or(orch.config.default_idle_minutes)
            .max(1);
        if since.elapsed() >= Duration::from_secs(u64::try_from(idle).unwrap_or(10) * 60) {
            tracing::info!(server = %id, minutes = idle, "no players; putting the server to sleep");
            let orch = orch.clone();
            let id = id.clone();
            // Run outside this task: stopping aborts the monitor tasks.
            tokio::spawn(async move {
                if let Err(e) = orch.stop(&id, true).await {
                    orch.report_failure(&id, "sleep", &e);
                }
            });
            return;
        }
    }
}

fn publish_players(orch: &Orchestrator, id: &str, players: Players) {
    orch.runtime(id).state().players = Some(players);
    orch.events.publish(Event::ServerPlayers {
        server_id: id.to_owned(),
        online: players.online,
        max: players.max,
    });
}

async fn stream_stats(orch: Arc<Orchestrator>, id: String, container: String) {
    let options = StatsOptionsBuilder::default().stream(true).build();
    let mut stream = orch.docker.stats(&container, Some(options));
    while let Some(Ok(stats)) = stream.next().await {
        let Some(mut sample) = sample_from(&stats) else {
            continue;
        };
        {
            let rt = orch.runtime(&id);
            let state = rt.state();
            sample.players_online = state.players.map(|p| p.online);
            sample.tick = state.tick;
        }
        orch.record_stats(&id, sample);
        orch.events.publish(Event::ServerStats {
            server_id: id.clone(),
            cpu_percent: sample.cpu_percent,
            memory_bytes: sample.memory_bytes,
            memory_limit_bytes: sample.memory_limit_bytes,
            net_rx_bytes: sample.net_rx_bytes,
            net_tx_bytes: sample.net_tx_bytes,
            disk_read_bytes: sample.disk_read_bytes,
            disk_write_bytes: sample.disk_write_bytes,
            players_online: sample.players_online,
            mspt: sample.tick.map(|t| t.mspt),
            mspt_p95: sample.tick.and_then(|t| t.mspt_p95),
            tps: sample.tick.map(|t| t.tps),
        });
    }
}

/// Converts a Docker stats frame to a sample. CPU % is relative to one
/// core (Docker's convention): `Δcontainer / Δsystem × cores × 100`.
pub(crate) fn sample_from(stats: &ContainerStatsResponse) -> Option<StatSample> {
    let cpu = stats.cpu_stats.as_ref()?;
    let pre = stats.precpu_stats.as_ref();
    let total = cpu.cpu_usage.as_ref()?.total_usage?;
    let pre_total = pre
        .and_then(|p| p.cpu_usage.as_ref())
        .and_then(|u| u.total_usage)
        .unwrap_or(0);
    let system = cpu.system_cpu_usage.unwrap_or(0);
    let pre_system = pre.and_then(|p| p.system_cpu_usage).unwrap_or(0);
    let cores = f64::from(cpu.online_cpus.unwrap_or(1).max(1));
    let (cpu_delta, system_delta) = (
        total.saturating_sub(pre_total),
        system.saturating_sub(pre_system),
    );
    let cpu_percent = if system_delta > 0 && pre_total > 0 {
        cpu_delta as f64 / system_delta as f64 * cores * 100.0
    } else {
        0.0
    };

    let memory = stats.memory_stats.as_ref();
    let usage = memory.and_then(|m| m.usage).unwrap_or(0);
    // Exclude reclaimable page cache: cgroup v2 `inactive_file`, v1 `cache`.
    let cache = memory
        .and_then(|m| m.stats.as_ref())
        .and_then(|s| s.get("inactive_file").or_else(|| s.get("cache")).copied())
        .unwrap_or(0);
    let (rx, tx) = stats
        .networks
        .as_ref()
        .map(|n| {
            n.values().fold((0, 0), |(r, t), v| {
                (r + v.rx_bytes.unwrap_or(0), t + v.tx_bytes.unwrap_or(0))
            })
        })
        .unwrap_or((0, 0));
    // cgroup v2 says `read`/`write`, v1 `Read`/`Write`.
    let (disk_read, disk_write) = stats
        .blkio_stats
        .as_ref()
        .and_then(|b| b.io_service_bytes_recursive.as_ref())
        .map(|entries| {
            entries.iter().fold((0, 0), |(r, w), e| {
                let value = e.value.unwrap_or(0);
                match e.op.as_deref().map(str::to_ascii_lowercase).as_deref() {
                    Some("read") => (r + value, w),
                    Some("write") => (r, w + value),
                    _ => (r, w),
                }
            })
        })
        .unwrap_or((0, 0));
    Some(StatSample {
        at_ms: now_ms(),
        cpu_percent,
        memory_bytes: usage.saturating_sub(cache),
        memory_limit_bytes: memory.and_then(|m| m.limit).unwrap_or(0),
        net_rx_bytes: rx,
        net_tx_bytes: tx,
        disk_read_bytes: disk_read,
        disk_write_bytes: disk_write,
        players_online: None,
        tick: None,
    })
}

async fn stream_logs(orch: Arc<Orchestrator>, rt: Arc<Runtime>, container: String, adopted: bool) {
    // After adoption, backfill recent output for late joiners.
    let tail = if adopted { "500" } else { "0" };
    let options = LogsOptionsBuilder::default()
        .follow(true)
        .stdout(true)
        .stderr(true)
        .tail(tail)
        .build();
    let mut stream = orch.docker.logs(&container, Some(options));
    let mut partial = String::new();
    while let Some(Ok(output)) = stream.next().await {
        let chunk = match output {
            LogOutput::StdOut { message }
            | LogOutput::StdErr { message }
            | LogOutput::Console { message } => message,
            LogOutput::StdIn { .. } => continue,
        };
        partial.push_str(&String::from_utf8_lossy(&chunk));
        while let Some(pos) = partial.find('\n') {
            let line: String = partial.drain(..=pos).collect();
            rt.logs.push(visible(&line));
        }
    }
    if !partial.is_empty() {
        rt.logs.push(visible(&partial));
    }
}

/// What a terminal would show for a raw output line. Servers attached to a
/// TTY redraw their `>` prompt before each log line with a carriage return
/// and an erase-line code (`>....\r\x1b[K[12:00:00 INFO]: …`); keep only
/// the text after the last carriage return, without the erase code. Colors
/// stay for the console to render.
pub(crate) fn visible(raw: &str) -> &str {
    let line = raw.trim_end_matches(['\n', '\r']);
    let line = line.rsplit('\r').next().unwrap_or(line);
    line.trim_start_matches("\x1b[K")
}

#[cfg(test)]
mod tests {
    #[test]
    fn prompt_redraws_are_removed() {
        use super::visible;
        assert_eq!(
            visible(">....\r\x1b[K[18:37:07 INFO]: Done (3.1s)!\r\n"),
            "[18:37:07 INFO]: Done (3.1s)!"
        );
        assert_eq!(visible("[INFO] plain\n"), "[INFO] plain");
        assert_eq!(
            visible("\x1b[33mwarn\x1b[0m"),
            "\x1b[33mwarn\x1b[0m",
            "colors kept"
        );
    }

    use std::collections::HashMap;

    use bollard::models::{
        ContainerCpuStats, ContainerCpuUsage, ContainerMemoryStats, ContainerNetworkStats,
    };

    use super::*;

    fn cpu(total: u64, system: u64) -> ContainerCpuStats {
        ContainerCpuStats {
            cpu_usage: Some(ContainerCpuUsage {
                total_usage: Some(total),
                ..Default::default()
            }),
            system_cpu_usage: Some(system),
            online_cpus: Some(4),
            ..Default::default()
        }
    }

    #[test]
    fn computes_cpu_memory_and_network() {
        let stats = ContainerStatsResponse {
            cpu_stats: Some(cpu(3_000, 20_000)),
            precpu_stats: Some(cpu(1_000, 10_000)),
            memory_stats: Some(ContainerMemoryStats {
                usage: Some(1_000_000),
                limit: Some(4_000_000),
                stats: Some(HashMap::from([("inactive_file".to_owned(), 200_000)])),
                ..Default::default()
            }),
            networks: Some(HashMap::from([
                (
                    "eth0".to_owned(),
                    ContainerNetworkStats {
                        rx_bytes: Some(10),
                        tx_bytes: Some(20),
                        ..Default::default()
                    },
                ),
                (
                    "eth1".to_owned(),
                    ContainerNetworkStats {
                        rx_bytes: Some(5),
                        tx_bytes: Some(5),
                        ..Default::default()
                    },
                ),
            ])),
            ..Default::default()
        };
        let s = sample_from(&stats).unwrap();
        // 2000 / 10000 × 4 cores × 100 = 80%
        assert!((s.cpu_percent - 80.0).abs() < 1e-9);
        assert_eq!((s.memory_bytes, s.memory_limit_bytes), (800_000, 4_000_000));
        assert_eq!((s.net_rx_bytes, s.net_tx_bytes), (15, 25));
    }

    #[test]
    fn first_frame_without_previous_cpu_reads_zero() {
        let stats = ContainerStatsResponse {
            cpu_stats: Some(cpu(3_000, 20_000)),
            precpu_stats: Some(ContainerCpuStats::default()),
            ..Default::default()
        };
        assert_eq!(sample_from(&stats).unwrap().cpu_percent, 0.0);
    }
}
