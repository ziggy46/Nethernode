//! Servers' resource use over time (see migration 0013): per-minute and
//! per-hour summaries, added to as samples arrive.

use crate::{Result, Store};

/// A summary of one period of a server's samples.
#[derive(Debug, Clone, Default, PartialEq, sqlx::FromRow)]
pub struct StatPeriod {
    /// Start, unix ms.
    pub at_ms: i64,
    /// Samples taken.
    pub samples: i64,
    /// Sum of CPU use (100 = one core).
    pub cpu_sum: f64,
    /// Peak CPU use.
    pub cpu_max: f64,
    /// Sum of memory in use.
    pub memory_sum: i64,
    /// Peak memory in use.
    pub memory_max: i64,
    /// The memory limit.
    pub memory_limit: i64,
    /// Bytes received during the period.
    pub net_rx: i64,
    /// Bytes sent.
    pub net_tx: i64,
    /// Bytes read from disk.
    pub disk_read: i64,
    /// Bytes written to disk.
    pub disk_write: i64,
    /// Most players online at once, if known.
    pub players_max: Option<i64>,
    /// Samples with tick timings.
    pub tick_samples: i64,
    /// Sum of milliseconds per tick.
    pub mspt_sum: f64,
    /// Slowest milliseconds per tick.
    pub mspt_max: Option<f64>,
    /// Sum of ticks per second.
    pub tps_sum: f64,
    /// Lowest ticks per second.
    pub tps_min: Option<f64>,
    /// Samples taken while the server was paused for want of players.
    pub paused_samples: i64,
}

impl Store {
    /// Adds `stats` to the server's `period`-second summary starting at
    /// `stats.at_ms`, creating it if needed.
    pub async fn add_stats(&self, server_id: &str, period: i64, stats: &StatPeriod) -> Result<()> {
        sqlx::query!(
            r#"INSERT INTO server_stats (server_id, period, at_ms, samples, cpu_sum, cpu_max,
                   memory_sum, memory_max, memory_limit, net_rx, net_tx, disk_read, disk_write,
                   players_max, tick_samples, mspt_sum, mspt_max, tps_sum, tps_min, paused_samples)
               VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17,
                   ?18, ?19, ?20)
               ON CONFLICT (server_id, period, at_ms) DO UPDATE SET
                   samples = samples + excluded.samples,
                   cpu_sum = cpu_sum + excluded.cpu_sum,
                   cpu_max = max(cpu_max, excluded.cpu_max),
                   memory_sum = memory_sum + excluded.memory_sum,
                   memory_max = max(memory_max, excluded.memory_max),
                   memory_limit = excluded.memory_limit,
                   net_rx = net_rx + excluded.net_rx,
                   net_tx = net_tx + excluded.net_tx,
                   disk_read = disk_read + excluded.disk_read,
                   disk_write = disk_write + excluded.disk_write,
                   players_max = max(coalesce(players_max, excluded.players_max),
                                     coalesce(excluded.players_max, players_max)),
                   tick_samples = tick_samples + excluded.tick_samples,
                   mspt_sum = mspt_sum + excluded.mspt_sum,
                   mspt_max = max(coalesce(mspt_max, excluded.mspt_max),
                                  coalesce(excluded.mspt_max, mspt_max)),
                   tps_sum = tps_sum + excluded.tps_sum,
                   tps_min = min(coalesce(tps_min, excluded.tps_min),
                                 coalesce(excluded.tps_min, tps_min)),
                   paused_samples = paused_samples + excluded.paused_samples"#,
            server_id,
            period,
            stats.at_ms,
            stats.samples,
            stats.cpu_sum,
            stats.cpu_max,
            stats.memory_sum,
            stats.memory_max,
            stats.memory_limit,
            stats.net_rx,
            stats.net_tx,
            stats.disk_read,
            stats.disk_write,
            stats.players_max,
            stats.tick_samples,
            stats.mspt_sum,
            stats.mspt_max,
            stats.tps_sum,
            stats.tps_min,
            stats.paused_samples,
        )
        .execute(self.pool())
        .await?;
        Ok(())
    }

    /// The server's `period`-second summaries since `since_ms`, combined
    /// into buckets of `bucket_ms`, oldest first. Each bucket's `at_ms` is
    /// its start.
    pub async fn stats_since(
        &self,
        server_id: &str,
        period: i64,
        since_ms: i64,
        bucket_ms: i64,
    ) -> Result<Vec<StatPeriod>> {
        let bucket_ms = bucket_ms.max(1);
        let rows = sqlx::query_as!(
            StatPeriod,
            r#"SELECT (at_ms / ?4) * ?4 AS "at_ms!: i64",
                   sum(samples) AS "samples!: i64",
                   sum(cpu_sum) AS "cpu_sum!: f64",
                   max(cpu_max) AS "cpu_max!: f64",
                   sum(memory_sum) AS "memory_sum!: i64",
                   max(memory_max) AS "memory_max!: i64",
                   max(memory_limit) AS "memory_limit!: i64",
                   sum(net_rx) AS "net_rx!: i64",
                   sum(net_tx) AS "net_tx!: i64",
                   sum(disk_read) AS "disk_read!: i64",
                   sum(disk_write) AS "disk_write!: i64",
                   max(players_max) AS "players_max: i64",
                   sum(tick_samples) AS "tick_samples!: i64",
                   sum(mspt_sum) AS "mspt_sum!: f64",
                   max(mspt_max) AS "mspt_max: f64",
                   sum(tps_sum) AS "tps_sum!: f64",
                   min(tps_min) AS "tps_min: f64",
                   sum(paused_samples) AS "paused_samples!: i64"
               FROM server_stats
               WHERE server_id = ?1 AND period = ?2 AND at_ms >= ?3
               GROUP BY 1
               ORDER BY 1"#,
            server_id,
            period,
            since_ms,
            bucket_ms,
        )
        .fetch_all(self.pool())
        .await?;
        Ok(rows)
    }

    /// Deletes `period`-second summaries that started before `before_ms`.
    pub async fn prune_stats(&self, period: i64, before_ms: i64) -> Result<u64> {
        let done = sqlx::query!(
            "DELETE FROM server_stats WHERE period = ?1 AND at_ms < ?2",
            period,
            before_ms,
        )
        .execute(self.pool())
        .await?;
        Ok(done.rows_affected())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::servers::test_server;
    use crate::test_store;

    fn minute(at_ms: i64, cpu: f64, players: Option<i64>, tps: Option<f64>) -> StatPeriod {
        StatPeriod {
            at_ms,
            samples: 60,
            cpu_sum: cpu * 60.0,
            cpu_max: cpu,
            memory_sum: 60 * 1000,
            memory_max: 1000,
            memory_limit: 4000,
            net_rx: 10,
            disk_write: 5,
            players_max: players,
            tick_samples: i64::from(tps.is_some()) * 60,
            tps_sum: tps.unwrap_or(0.0) * 60.0,
            tps_min: tps,
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn periods_add_up_and_combine_into_buckets() {
        let store = test_store().await;
        let server = store
            .insert_server(test_server("smp", 30001))
            .await
            .unwrap();
        let id = &server.id;
        store
            .add_stats(id, 60, &minute(0, 10.0, None, Some(20.0)))
            .await
            .unwrap();
        // More samples for the same minute merge into it.
        store
            .add_stats(id, 60, &minute(0, 50.0, Some(3), Some(12.0)))
            .await
            .unwrap();
        store
            .add_stats(id, 60, &minute(60_000, 30.0, Some(1), None))
            .await
            .unwrap();
        store
            .add_stats(id, 60, &minute(300_000, 5.0, None, None))
            .await
            .unwrap();

        let rows = store.stats_since(id, 60, 0, 60_000).await.unwrap();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].samples, 120);
        assert_eq!(rows[0].cpu_max, 50.0);
        assert_eq!(rows[0].players_max, Some(3));
        assert_eq!(rows[0].tps_min, Some(12.0));
        assert_eq!(rows[0].net_rx, 20);

        let buckets = store.stats_since(id, 60, 0, 300_000).await.unwrap();
        assert_eq!(
            buckets.iter().map(|b| b.at_ms).collect::<Vec<_>>(),
            [0, 300_000]
        );
        assert_eq!(buckets[0].samples, 180);
        assert_eq!(buckets[0].players_max, Some(3));
        assert_eq!(buckets[1].players_max, None);

        assert_eq!(store.stats_since(id, 3600, 0, 60_000).await.unwrap(), []);
        assert_eq!(store.prune_stats(60, 300_000).await.unwrap(), 2);
        assert_eq!(store.stats_since(id, 60, 0, 60_000).await.unwrap().len(), 1);
    }
}
