//! History: SQLite, decimated so it stays small.
//!
//! Three series are kept:
//! - **space**: total and available bytes per volume, one row per sample;
//! - **writes**: bytes written per process name during each sample interval;
//! - **growth**: bytes each owner folder grew (or shrank) per period.
//!
//! Rows start at 5-minute resolution. After a day they are folded into hourly
//! rows, after a month into daily rows, and daily rows are dropped after
//! about a year. Free space keeps the *last* value of each bucket (a level);
//! writes and growth are *summed* (quantities). The whole history of a Mac stays well
//! under a few MB, which is part of the watcher's budget.

use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result};
use rusqlite::{Connection, params};

use crate::incidents::Incident;

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

/// One episode of an incident: from when it first held to when it stopped.
#[derive(Debug, Clone, PartialEq)]
pub struct Episode {
    pub opened: i64,
    pub updated: i64,
    pub closed: Option<i64>,
    /// The incident as last seen.
    pub incident: Incident,
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
             CREATE TABLE IF NOT EXISTS growth (
                 res   INTEGER NOT NULL,
                 ts    INTEGER NOT NULL,
                 owner TEXT    NOT NULL,
                 bytes INTEGER NOT NULL,
                 PRIMARY KEY (res, ts, owner)
             ) WITHOUT ROWID;
             CREATE TABLE IF NOT EXISTS incidents (
                 kind    TEXT    NOT NULL,
                 subject TEXT    NOT NULL,
                 opened  INTEGER NOT NULL,
                 updated INTEGER NOT NULL,
                 closed  INTEGER,
                 detail  TEXT    NOT NULL,
                 PRIMARY KEY (kind, subject, opened)
             ) WITHOUT ROWID;
             CREATE TABLE IF NOT EXISTS meta (
                 key   TEXT PRIMARY KEY,
                 value TEXT NOT NULL
             );",
        )?;
        Ok(Store { conn })
    }

    pub fn add_space(&self, vol: &str, p: SpacePoint) -> Result<()> {
        self.record(vol, p, &[], &[], &[])
    }

    /// Growth of each owner during the period ending at `ts`.
    pub fn add_growth(&self, ts: i64, growth: &[(String, i64)]) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        insert_growth(&tx, ts, growth)?;
        tx.commit()?;
        Ok(())
    }

    /// Bytes written by each process name during the interval ending at `ts`.
    pub fn add_writes(&self, ts: i64, writes: &[(String, u64)]) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        insert_writes(&tx, ts, writes)?;
        tx.commit()?;
        Ok(())
    }

    /// One whole sample in a single transaction: free space, writes during
    /// the interval, owner growth (when a period ended) and metadata. One
    /// commit instead of several roughly halves the bytes each sample costs
    /// (WAL frames are per commit).
    pub fn record(
        &self,
        vol: &str,
        p: SpacePoint,
        writes: &[(String, u64)],
        growth: &[(String, i64)],
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
        insert_growth(&tx, p.ts, growth)?;
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

    /// Every process name's per-interval writes since `since`, oldest first.
    pub fn writes_series(&self, since: i64) -> Result<HashMap<String, Vec<(i64, u64)>>> {
        let mut st = self.conn.prepare_cached(
            "SELECT name, ts, SUM(bytes) FROM writes WHERE ts >= ?1 GROUP BY name, ts
             ORDER BY name, ts",
        )?;
        let mut out: HashMap<String, Vec<(i64, u64)>> = HashMap::new();
        let mut rows = st.query(params![since])?;
        while let Some(r) = rows.next()? {
            out.entry(r.get(0)?)
                .or_default()
                .push((r.get(1)?, r.get::<_, i64>(2)? as u64));
        }
        Ok(out)
    }

    /// Record which incidents hold at `now`. One that holds and has no open
    /// episode opens one; one that stops holding is closed once it hasn't
    /// held for `grace` seconds. Returns the episodes opened now, so each
    /// episode is announced once.
    pub fn update_incidents(
        &self,
        now: i64,
        active: &[Incident],
        grace: i64,
    ) -> Result<Vec<Incident>> {
        let tx = self.conn.unchecked_transaction()?;
        let mut opened = Vec::new();
        for i in active {
            let detail = serde_json::to_string(i)?;
            let n = tx
                .prepare_cached(
                    "UPDATE incidents SET updated = ?3, detail = ?4
                     WHERE kind = ?1 AND subject = ?2 AND closed IS NULL",
                )?
                .execute(params![i.kind.as_str(), i.subject, now, detail])?;
            if n == 0 {
                tx.prepare_cached(
                    "INSERT INTO incidents (kind, subject, opened, updated, detail)
                     VALUES (?1, ?2, ?3, ?3, ?4)",
                )?
                .execute(params![i.kind.as_str(), i.subject, now, detail])?;
                opened.push(i.clone());
            }
        }
        tx.prepare_cached(
            "UPDATE incidents SET closed = updated WHERE closed IS NULL AND updated < ?1",
        )?
        .execute(params![now - grace])?;
        tx.commit()?;
        Ok(opened)
    }

    /// Episodes open now or updated since `since`, open ones first, then the
    /// most recent.
    pub fn incidents_since(&self, since: i64) -> Result<Vec<Episode>> {
        let mut st = self.conn.prepare_cached(
            "SELECT opened, updated, closed, detail FROM incidents
             WHERE closed IS NULL OR updated >= ?1
             ORDER BY closed IS NOT NULL, updated DESC",
        )?;
        let rows = st.query_map(params![since], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get::<_, String>(3)?))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (opened, updated, closed, detail) = row?;
            out.push(Episode {
                opened,
                updated,
                closed,
                incident: serde_json::from_str(&detail)?,
            });
        }
        Ok(out)
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

    /// Net growth per owner since `since`, largest first.
    pub fn growth_since(&self, since: i64) -> Result<Vec<(String, i64)>> {
        let mut st = self.conn.prepare_cached(
            "SELECT owner, SUM(bytes) AS b FROM growth WHERE ts >= ?1 GROUP BY owner
             ORDER BY b DESC",
        )?;
        let rows = st.query_map(params![since], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Every owner's per-period growth since `since`, oldest first.
    pub fn growth_series(&self, since: i64) -> Result<HashMap<String, Vec<(i64, i64)>>> {
        let mut st = self.conn.prepare_cached(
            "SELECT owner, ts, SUM(bytes) FROM growth WHERE ts >= ?1 GROUP BY owner, ts
             ORDER BY owner, ts",
        )?;
        let mut out: HashMap<String, Vec<(i64, i64)>> = HashMap::new();
        let mut rows = st.query(params![since])?;
        while let Some(r) = rows.next()? {
            out.entry(r.get(0)?)
                .or_default()
                .push((r.get(1)?, r.get(2)?));
        }
        Ok(out)
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
            // Writes and growth are quantities: sum each bucket.
            for (table, key) in QUANTITIES {
                tx.execute(
                    &format!(
                        "INSERT INTO {table} (res, ts, {key}, bytes)
                         SELECT ?2, (ts / ?2) * ?2 AS b, {key}, SUM(bytes)
                         FROM {table} WHERE res = ?1 AND ts < ?3 GROUP BY b, {key}
                         ON CONFLICT (res, ts, {key}) DO UPDATE SET bytes = bytes + excluded.bytes"
                    ),
                    params![fine.res, coarse.res, cut],
                )?;
                tx.execute(
                    &format!("DELETE FROM {table} WHERE res = ?1 AND ts < ?2"),
                    params![fine.res, cut],
                )?;
            }
        }
        let last = TIERS[TIERS.len() - 1];
        tx.execute(
            "DELETE FROM space WHERE res = ?1 AND ts < ?2",
            params![last.res, now - last.keep],
        )?;
        for (table, _) in QUANTITIES {
            tx.execute(
                &format!("DELETE FROM {table} WHERE res = ?1 AND ts < ?2"),
                params![last.res, now - last.keep],
            )?;
        }
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
        for table in ["space", "writes", "growth"] {
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

/// Tables of per-period quantities, and their key column.
const QUANTITIES: [(&str, &str); 2] = [("writes", "name"), ("growth", "owner")];

fn insert_growth(conn: &Connection, ts: i64, growth: &[(String, i64)]) -> Result<()> {
    let mut st = conn.prepare_cached(
        "INSERT INTO growth (res, ts, owner, bytes) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT (res, ts, owner) DO UPDATE SET bytes = bytes + excluded.bytes",
    )?;
    for (owner, bytes) in growth {
        st.execute(params![TIERS[0].res, ts, owner, bytes])?;
    }
    Ok(())
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
    fn growth_is_signed_summed_and_compacted() {
        let s = Store::open_in_memory().unwrap();
        let day = 86_400;
        for i in 0..(2 * day / 1800) {
            let ts = i * 1800;
            s.add_growth(ts, &[("/a".into(), 10), ("/b".into(), -3)])
                .unwrap();
        }
        let net = s.growth_since(0).unwrap();
        assert_eq!(net, [("/a".to_string(), 960), ("/b".to_string(), -288)]);
        s.compact(2 * day).unwrap();
        assert_eq!(s.growth_since(0).unwrap(), net, "sums survive compaction");
        let series = s.growth_series(0).unwrap();
        // First day hourly (2 periods each), second day per period.
        assert_eq!(series["/a"].len(), 24 + 48);
        assert_eq!(series["/a"][0], (0, 20));
    }

    #[test]
    fn incident_episodes_open_once_and_close_after_the_grace() {
        use crate::incidents::{Incident, Kind};
        let s = Store::open_in_memory().unwrap();
        let inc = |amount| Incident {
            kind: Kind::Churn,
            subject: "OneDrive".into(),
            at: 0,
            window: 3600,
            amount,
            per_day: 0.0,
            fit: 0.0,
            size: None,
            full_in_days: None,
        };
        assert_eq!(s.update_incidents(100, &[inc(1)], 600).unwrap().len(), 1);
        assert!(
            s.update_incidents(200, &[inc(2)], 600).unwrap().is_empty(),
            "announced once"
        );
        // Stops holding: still open within the grace, closed after.
        s.update_incidents(500, &[], 600).unwrap();
        assert_eq!(s.incidents_since(0).unwrap()[0].closed, None);
        s.update_incidents(900, &[], 600).unwrap();
        let ep = s.incidents_since(0).unwrap();
        assert_eq!(ep.len(), 1);
        assert_eq!(
            (ep[0].opened, ep[0].updated, ep[0].closed),
            (100, 200, Some(200))
        );
        assert_eq!(ep[0].incident.amount, 2);
        // Holding again later is a new episode.
        assert_eq!(s.update_incidents(5000, &[inc(3)], 600).unwrap().len(), 1);
        assert_eq!(s.incidents_since(0).unwrap().len(), 2);
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
