//! History: SQLite, decimated so it stays small.
//!
//! Two series are kept:
//! - **space**: total and available bytes per volume, one row per sample;
//! - **writes**: bytes written per process name during each sample interval.
//!
//! Rows start at 5-minute resolution. After a day they are folded into hourly
//! rows, after a month into daily rows, and daily rows are dropped after
//! about a year. Free space keeps the *last* value of each bucket (a level);
//! writes are *summed* (a quantity). The whole history of a Mac stays well
//! under a few MB, which is part of the watcher's budget.

use std::path::Path;

use anyhow::{Context, Result};
use rusqlite::{Connection, params};

/// Resolutions and how long rows at each are kept before being folded into
/// the next one (or, for the last tier, dropped).
pub const TIERS: [Tier; 3] = [
    Tier {
        res: 300,
        keep: 86_400,
    },
    Tier {
        res: 3_600,
        keep: 31 * 86_400,
    },
    Tier {
        res: 86_400,
        keep: 400 * 86_400,
    },
];

#[derive(Debug, Clone, Copy)]
pub struct Tier {
    /// Seconds per row.
    pub res: i64,
    /// Seconds of history kept at this resolution.
    pub keep: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpacePoint {
    /// Unix seconds.
    pub ts: i64,
    pub total: u64,
    pub avail: u64,
}

pub struct Store {
    conn: Connection,
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        }
        let conn = Connection::open(path).with_context(|| format!("opening {}", path.display()))?;
        // Each page a sample touches is written to the WAL in full, and our
        // rows are tiny: 1 KB pages make a sample cost ~5 KB instead of ~19
        // KB with the default 4 KB pages. Only takes effect on a new file.
        conn.pragma_update(None, "page_size", 1024)?;
        // WAL keeps each sample to a few page writes.
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        Self::init(conn)
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Self> {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS space (
                 vol   TEXT    NOT NULL,
                 res   INTEGER NOT NULL,
                 ts    INTEGER NOT NULL,
                 total INTEGER NOT NULL,
                 avail INTEGER NOT NULL,
                 PRIMARY KEY (vol, res, ts)
             ) WITHOUT ROWID;
             CREATE TABLE IF NOT EXISTS writes (
                 res   INTEGER NOT NULL,
                 ts    INTEGER NOT NULL,
                 name  TEXT    NOT NULL,
                 bytes INTEGER NOT NULL,
                 PRIMARY KEY (res, ts, name)
             ) WITHOUT ROWID;
             CREATE TABLE IF NOT EXISTS meta (
                 key   TEXT PRIMARY KEY,
                 value TEXT NOT NULL
             );",
        )?;
        Ok(Store { conn })
    }

    pub fn add_space(&self, vol: &str, p: SpacePoint) -> Result<()> {
        self.record(vol, p, &[], &[])
    }

    /// Bytes written by each process name during the interval ending at `ts`.
    pub fn add_writes(&self, ts: i64, writes: &[(String, u64)]) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        insert_writes(&tx, ts, writes)?;
        tx.commit()?;
        Ok(())
    }

    /// One whole sample in a single transaction: free space, writes during
    /// the interval and metadata. One commit instead of three roughly halves
    /// the bytes each sample costs (WAL frames are per commit).
    pub fn record(
        &self,
        vol: &str,
        p: SpacePoint,
        writes: &[(String, u64)],
        meta: &[(&str, &str)],
    ) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        tx.prepare_cached(
            "INSERT OR REPLACE INTO space (vol, res, ts, total, avail) VALUES (?1, ?2, ?3, ?4, ?5)",
        )?
        .execute(params![
            vol,
            TIERS[0].res,
            p.ts,
            p.total as i64,
            p.avail as i64
        ])?;
        insert_writes(&tx, p.ts, writes)?;
        for (k, v) in meta {
            tx.prepare_cached("INSERT OR REPLACE INTO meta (key, value) VALUES (?1, ?2)")?
                .execute(params![k, v])?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Free-space history of `vol` since `since`, oldest first, across all
    /// resolutions (they never overlap in time).
    pub fn space_since(&self, vol: &str, since: i64) -> Result<Vec<SpacePoint>> {
        let mut st = self.conn.prepare_cached(
            "SELECT ts, total, avail FROM space WHERE vol = ?1 AND ts >= ?2 ORDER BY ts",
        )?;
        let rows = st.query_map(params![vol, since], |r| {
            Ok(SpacePoint {
                ts: r.get(0)?,
                total: r.get::<_, i64>(1)? as u64,
                avail: r.get::<_, i64>(2)? as u64,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Total bytes written per process name since `since`, largest first.
    pub fn writes_since(&self, since: i64) -> Result<Vec<(String, u64)>> {
        let mut st = self.conn.prepare_cached(
            "SELECT name, SUM(bytes) AS b FROM writes WHERE ts >= ?1 GROUP BY name ORDER BY b DESC",
        )?;
        let rows = st.query_map(params![since], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)? as u64))
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Per-interval writes of one process name since `since`, oldest first.
    pub fn writes_of(&self, name: &str, since: i64) -> Result<Vec<(i64, u64)>> {
        let mut st = self.conn.prepare_cached(
            "SELECT ts, SUM(bytes) FROM writes WHERE name = ?1 AND ts >= ?2 GROUP BY ts ORDER BY ts",
        )?;
        let rows = st.query_map(params![name, since], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)? as u64))
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Fold rows older than their tier's retention into the next tier and
    /// drop what falls off the last one. Only whole buckets are folded, so a
    /// bucket is never split between two rows of the coarser tier.
    pub fn compact(&self, now: i64) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        for pair in TIERS.windows(2) {
            let (fine, coarse) = (pair[0], pair[1]);
            let cut = (now - fine.keep).div_euclid(coarse.res) * coarse.res;
            // Free space is a level: keep the last sample of each bucket.
            tx.execute(
                "INSERT OR REPLACE INTO space (vol, res, ts, total, avail)
                 SELECT s.vol, ?2, (s.ts / ?2) * ?2, s.total, s.avail
                 FROM space s
                 WHERE s.res = ?1 AND s.ts < ?3
                   AND s.ts = (SELECT MAX(t.ts) FROM space t
                               WHERE t.vol = s.vol AND t.res = ?1 AND t.ts / ?2 = s.ts / ?2)",
                params![fine.res, coarse.res, cut],
            )?;
            tx.execute(
                "DELETE FROM space WHERE res = ?1 AND ts < ?2",
                params![fine.res, cut],
            )?;
            // Writes are a quantity: sum each bucket.
            tx.execute(
                "INSERT INTO writes (res, ts, name, bytes)
                 SELECT ?2, (ts / ?2) * ?2 AS b, name, SUM(bytes)
                 FROM writes WHERE res = ?1 AND ts < ?3 GROUP BY b, name
                 ON CONFLICT (res, ts, name) DO UPDATE SET bytes = bytes + excluded.bytes",
                params![fine.res, coarse.res, cut],
            )?;
            tx.execute(
                "DELETE FROM writes WHERE res = ?1 AND ts < ?2",
                params![fine.res, cut],
            )?;
        }
        let last = TIERS[TIERS.len() - 1];
        tx.execute(
            "DELETE FROM space WHERE res = ?1 AND ts < ?2",
            params![last.res, now - last.keep],
        )?;
        tx.execute(
            "DELETE FROM writes WHERE res = ?1 AND ts < ?2",
            params![last.res, now - last.keep],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn set_meta(&self, key: &str, value: &str) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO meta (key, value) VALUES (?1, ?2)",
            params![key, value],
        )?;
        Ok(())
    }

    pub fn meta(&self, key: &str) -> Result<Option<String>> {
        let mut st = self
            .conn
            .prepare_cached("SELECT value FROM meta WHERE key = ?1")?;
        let mut rows = st.query(params![key])?;
        Ok(match rows.next()? {
            Some(r) => Some(r.get(0)?),
            None => None,
        })
    }

    /// Rows per (table, resolution), for tests and `mdif watch --status`.
    pub fn row_counts(&self) -> Result<Vec<(String, i64, i64)>> {
        let mut out = Vec::new();
        for table in ["space", "writes"] {
            let mut st = self.conn.prepare(&format!(
                "SELECT res, COUNT(*) FROM {table} GROUP BY res ORDER BY res"
            ))?;
            let rows = st.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))?;
            for row in rows {
                let (res, n) = row?;
                out.push((table.to_string(), res, n));
            }
        }
        Ok(out)
    }
}

fn insert_writes(conn: &Connection, ts: i64, writes: &[(String, u64)]) -> Result<()> {
    let mut st = conn.prepare_cached(
        "INSERT INTO writes (res, ts, name, bytes) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT (res, ts, name) DO UPDATE SET bytes = bytes + excluded.bytes",
    )?;
    for (name, bytes) in writes {
        st.execute(params![TIERS[0].res, ts, name, *bytes as i64])?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const GB: u64 = 1_000_000_000;

    fn point(ts: i64, avail: u64) -> SpacePoint {
        SpacePoint {
            ts,
            total: 500 * GB,
            avail,
        }
    }

    #[test]
    fn space_round_trips_in_order() {
        let s = Store::open_in_memory().unwrap();
        s.add_space("/", point(600, 40 * GB)).unwrap();
        s.add_space("/", point(300, 41 * GB)).unwrap();
        s.add_space("/other", point(300, 1)).unwrap();
        let got = s.space_since("/", 0).unwrap();
        assert_eq!(got, [point(300, 41 * GB), point(600, 40 * GB)]);
    }

    #[test]
    fn writes_accumulate_per_name() {
        let s = Store::open_in_memory().unwrap();
        s.add_writes(300, &[("OneDrive".into(), 5), ("cargo".into(), 1)])
            .unwrap();
        s.add_writes(600, &[("OneDrive".into(), 7)]).unwrap();
        s.add_writes(600, &[("OneDrive".into(), 1)]).unwrap();
        assert_eq!(
            s.writes_since(0).unwrap(),
            [("OneDrive".to_string(), 13), ("cargo".to_string(), 1)]
        );
        assert_eq!(s.writes_of("OneDrive", 0).unwrap(), [(300, 5), (600, 8)]);
    }

    #[test]
    fn compaction_keeps_levels_and_sums_quantities() {
        let s = Store::open_in_memory().unwrap();
        let day = 86_400;
        // Two days of 5-minute samples: free space falling 1 MB per sample,
        // and 10 bytes written per sample.
        let n = 2 * day / 300;
        for i in 0..n {
            let ts = i * 300;
            s.add_space("/", point(ts, 100 * GB - i as u64 * 1_000_000))
                .unwrap();
            s.add_writes(ts, &[("p".into(), 10)]).unwrap();
        }
        let before = s.space_since("/", 0).unwrap();
        let written: u64 = s.writes_since(0).unwrap()[0].1;
        let now = 2 * day;
        s.compact(now).unwrap();

        let after = s.space_since("/", 0).unwrap();
        // The first day is hourly now, the last day still 5-minute.
        assert!(after.len() < before.len());
        assert_eq!(after.len(), 24 + (day / 300) as usize);
        // Each hourly row holds the level at the end of its hour.
        let first_hour = after[0];
        assert_eq!(first_hour.ts, 0);
        assert_eq!(first_hour.avail, 100 * GB - 11 * 1_000_000);
        // The most recent sample is untouched.
        assert_eq!(after.last(), before.last());
        // Writes are summed, never lost.
        assert_eq!(s.writes_since(0).unwrap()[0].1, written);
        // Compacting again changes nothing.
        s.compact(now).unwrap();
        assert_eq!(s.space_since("/", 0).unwrap(), after);
        assert_eq!(s.writes_since(0).unwrap()[0].1, written);
    }

    #[test]
    fn history_past_the_last_tier_is_dropped() {
        let s = Store::open_in_memory().unwrap();
        s.add_space("/", point(0, GB)).unwrap();
        s.add_writes(0, &[("p".into(), 1)]).unwrap();
        s.compact(500 * 86_400).unwrap();
        assert!(s.space_since("/", 0).unwrap().is_empty());
        assert!(s.writes_since(0).unwrap().is_empty());
    }

    #[test]
    fn meta_round_trips() {
        let s = Store::open_in_memory().unwrap();
        assert_eq!(s.meta("k").unwrap(), None);
        s.set_meta("k", "v").unwrap();
        s.set_meta("k", "w").unwrap();
        assert_eq!(s.meta("k").unwrap().as_deref(), Some("w"));
    }
}
