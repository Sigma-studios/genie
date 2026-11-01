//! What was run, and for which query. Things used often and recently rise, and
//! what a given query led to last time rises most. Also Up in an empty input.

use crate::config;
use crate::matcher;
use rusqlite::Connection;
use rusqlite::params;
use std::collections::HashMap;
use std::path::Path;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

/// A use counts half as much after this many days.
const HALF_LIFE_DAYS: f64 = 30.0;

struct Use {
    time: i64,
    query: String,
}

pub struct History {
    db: Option<Connection>,
    uses: HashMap<(String, String), Vec<Use>>,
    /// Oldest first, without repeats.
    queries: Vec<String>,
    now: i64,
}

fn unix_now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

impl History {
    pub fn open() -> Self {
        let dir = config::data_home().join("genie");
        let _ = std::fs::create_dir_all(&dir);
        let albert = config::data_home().join("albert/albert.db");
        let db = Connection::open(dir.join("history.db"))
            .and_then(|db| {
                prepare(&db, &albert)?;
                Ok(db)
            })
            .inspect_err(|e| log::warn!("usage history isn't available: {e}"))
            .ok();
        Self::from_connection(db)
    }

    #[cfg(test)]
    pub fn in_memory() -> Self {
        let db = Connection::open_in_memory().unwrap();
        prepare(&db, Path::new("/does/not/exist")).unwrap();
        Self::from_connection(Some(db))
    }

    fn from_connection(db: Option<Connection>) -> Self {
        let mut history = Self { db, uses: HashMap::new(), queries: Vec::new(), now: unix_now() };
        let rows = history.db.as_ref().and_then(|db| load(db).ok()).unwrap_or_default();
        for (time, query, plugin, item) in rows {
            history.remember(time, query, plugin, item);
        }
        history
    }

    fn remember(&mut self, time: i64, query: String, plugin: String, item: String) {
        self.queries.retain(|q| *q != query);
        self.queries.push(query.clone());
        self.uses.entry((plugin, item)).or_default().push(Use { time, query: matcher::normalize(&query) });
    }

    pub fn record(&mut self, query: &str, plugin: &str, item: &str, action: &str) {
        let time = unix_now();
        self.now = time;
        if let Some(db) = &self.db {
            let result = db.execute(
                "INSERT INTO activation (time, query, plugin, item, action) VALUES (?, ?, ?, ?, ?)",
                params![time, query, plugin, item, action],
            );
            if let Err(e) = result {
                log::warn!("couldn't save usage: {e}");
            }
        }
        self.remember(time, query.to_string(), plugin.to_string(), item.to_string());
    }

    /// Between 0 and 0.75, to add to a match score.
    pub fn boost(&self, plugin: &str, item: &str, query: &str) -> f32 {
        let Some(uses) = self.uses.get(&(plugin.to_string(), item.to_string())) else { return 0.0 };
        let query = matcher::normalize(query);
        let mut weight = 0.0;
        let mut led_here = false;
        for used in uses {
            let days = (self.now - used.time).max(0) as f64 / 86400.0;
            weight += 0.5f64.powf(days / HALF_LIFE_DAYS);
            led_here |= !query.is_empty() && used.query.starts_with(&query);
        }
        let frequency = (weight / (weight + 4.0)) as f32 * 0.5;
        frequency + if led_here { 0.25 } else { 0.0 }
    }

    /// Previous query, `back` steps from the latest (0 is the latest).
    pub fn query(&self, back: usize) -> Option<&str> {
        self.queries.iter().rev().nth(back).map(String::as_str)
    }
}

fn prepare(db: &Connection, albert: &Path) -> rusqlite::Result<()> {
    let new = !db.prepare("SELECT 1 FROM sqlite_master WHERE name = 'activation'")?.exists([])?;
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS activation (
           time INTEGER NOT NULL, query TEXT NOT NULL, plugin TEXT NOT NULL, item TEXT NOT NULL, action TEXT NOT NULL
         )",
    )?;
    if new
        && albert.exists()
        && let Err(e) = import_albert(db, albert)
    {
        log::warn!("couldn't import Albert's history: {e}");
    }
    Ok(())
}

/// Once, when our table is new: Albert's usage carries over, so the ranking
/// doesn't start from nothing. Albert's database is only read.
fn import_albert(db: &Connection, albert: &Path) -> rusqlite::Result<()> {
    db.execute("ATTACH DATABASE ? AS albert", [format!("file:{}?mode=ro", albert.display())])?;
    let result = db.execute_batch(
        // Zed's items were <workspace>-<binary>-<time> there, and just the
        // workspace here. The timestamps were text.
        "INSERT INTO activation (time, query, plugin, item, action)
         SELECT CAST(strftime('%s', timestamp) AS INTEGER),
                COALESCE(query, ''),
                CASE extension_id
                  WHEN 'python.albert_zed_workspaces' THEN 'zed'
                  WHEN 'python.repertoire' THEN 'files'
                  WHEN 'system' THEN 'power'
                  ELSE extension_id
                END,
                CASE extension_id
                  WHEN 'python.albert_zed_workspaces' THEN substr(item_id, 1, instr(item_id, '-') - 1)
                  ELSE item_id
                END,
                COALESCE(action_id, '')
         FROM albert.activation
         WHERE extension_id IN ('applications', 'websearch', 'system', 'python.repertoire', 'python.albert_zed_workspaces')
           AND item_id IS NOT NULL
           AND strftime('%s', timestamp) IS NOT NULL
         ORDER BY timestamp",
    );
    db.execute("DETACH DATABASE albert", [])?;
    result
}

fn load(db: &Connection) -> rusqlite::Result<Vec<(i64, String, String, String)>> {
    let mut statement = db.prepare("SELECT time, query, plugin, item FROM activation ORDER BY rowid")?;
    let rows = statement.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?;
    rows.collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn memory() -> History {
        let db = Connection::open_in_memory().unwrap();
        prepare(&db, Path::new("/does/not/exist")).unwrap();
        History::from_connection(Some(db))
    }

    #[test]
    fn usage_raises_an_item_and_its_query_more() {
        let mut history = memory();
        assert_eq!(history.boost("apps", "firefox", "fi"), 0.0);
        history.record("fire", "apps", "firefox", "launch");
        let other_query = history.boost("apps", "firefox", "zzz");
        let same_query = history.boost("apps", "firefox", "fi");
        assert!(other_query > 0.0 && same_query > other_query);
        assert_eq!(history.boost("apps", "other", "fi"), 0.0);
    }

    #[test]
    fn old_uses_count_less() {
        let mut history = memory();
        history.remember(unix_now(), "".into(), "p".into(), "new".into());
        history.remember(unix_now() - 90 * 86400, "".into(), "p".into(), "old".into());
        assert!(history.boost("p", "new", "") > history.boost("p", "old", ""));
    }

    #[test]
    fn queries_recall_latest_first_without_repeats() {
        let mut history = memory();
        for query in ["a", "b", "a"] {
            history.record(query, "p", "i", "x");
        }
        assert_eq!(history.query(0), Some("a"));
        assert_eq!(history.query(1), Some("b"));
        assert_eq!(history.query(2), None);
    }

    #[test]
    fn albert_history_is_imported_once() {
        let dir = std::env::temp_dir().join(format!("genie-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("albert.db");
        let _ = std::fs::remove_file(&path);
        let albert = Connection::open(&path).unwrap();
        albert
            .execute_batch(
                "CREATE TABLE activation (timestamp INTEGER, query TEXT, extension_id, item_id TEXT, action_id TEXT);
                 INSERT INTO activation VALUES ('2026-04-12 13:38:23', 'disc', 'applications', 'discord', 'launch');
                 INSERT INTO activation VALUES ('2026-04-12 13:38:24', 'zd', 'python.albert_zed_workspaces', '1-zeditor-177', 'open');
                 INSERT INTO activation VALUES ('2026-04-12 13:38:25', 'x', 'pluginregistry', 'skip', 'x');",
            )
            .unwrap();
        drop(albert);
        let db = Connection::open_in_memory().unwrap();
        prepare(&db, &path).unwrap();
        prepare(&db, &path).unwrap();
        let rows = load(&db).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].2, "applications");
        assert_eq!((rows[1].2.as_str(), rows[1].3.as_str()), ("zed", "1"));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
