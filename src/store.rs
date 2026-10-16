//! `ggs.db`: logins, settings and chat lines. Chat rows carry their server as `host:port`.

use rusqlite::Connection;
use std::path::Path;
use std::time::{Duration, Instant, SystemTime};

const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// Each owner (the viewer, a chat) opens its own connection; WAL lets them write side by side.
pub fn open() -> rusqlite::Result<Connection> {
    open_at(&crate::data_file("ggs.db"))
}

fn open_at(path: &Path) -> rusqlite::Result<Connection> {
    let db = Connection::open(path)?;
    db.busy_timeout(BUSY_TIMEOUT)?;
    // Switching a new file to WAL fails as busy at once, without the busy handler, while another owner
    // switches it too.
    let deadline = Instant::now() + BUSY_TIMEOUT;
    loop {
        match db.execute_batch("PRAGMA journal_mode=WAL") {
            Err(rusqlite::Error::SqliteFailure(e, _))
                if e.code == rusqlite::ErrorCode::DatabaseBusy && Instant::now() < deadline =>
            {
                std::thread::sleep(Duration::from_millis(10));
            }
            result => break result?,
        }
    }
    db.execute_batch("PRAGMA synchronous=NORMAL")?;
    init(&db)?;
    Ok(db)
}

pub fn init(db: &Connection) -> rusqlite::Result<()> {
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS logins(host TEXT NOT NULL, port TEXT NOT NULL, login TEXT NOT NULL, rank INTEGER NOT NULL, PRIMARY KEY(host, port, login));
         CREATE TABLE IF NOT EXISTS saved_passwords(key TEXT PRIMARY KEY);
         CREATE TABLE IF NOT EXISTS settings(key TEXT PRIMARY KEY, value TEXT NOT NULL);
         CREATE TABLE IF NOT EXISTS chat(id INTEGER PRIMARY KEY, server TEXT NOT NULL, login TEXT NOT NULL, at TEXT NOT NULL, conversation TEXT NOT NULL, sender TEXT NOT NULL, line TEXT NOT NULL);
         CREATE INDEX IF NOT EXISTS chat_server ON chat(server, id);",
    )
}

/// The value under `key` in the settings table; empty when missing or unreadable.
pub fn setting(db: &Connection, key: &str) -> String {
    db.query_row("SELECT value FROM settings WHERE key = ?1", [key], |r| r.get(0)).unwrap_or_default()
}

pub fn put_setting(db: &Connection, key: &str, value: &str) -> rusqlite::Result<()> {
    db.execute(
        "INSERT INTO settings(key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        [key, value],
    )
    .map(drop)
}

/// Whether the app saved a password for this credential key.
pub fn password_saved(db: &Connection, key: &str) -> rusqlite::Result<bool> {
    db.query_row("SELECT EXISTS (SELECT 1 FROM saved_passwords WHERE key = ?1)", [key], |r| r.get(0))
}

pub fn mark_password_saved(db: &Connection, key: &str, saved: bool) -> rusqlite::Result<()> {
    let sql = if saved {
        "INSERT OR IGNORE INTO saved_passwords(key) VALUES (?1)"
    } else {
        "DELETE FROM saved_passwords WHERE key = ?1"
    };
    db.execute(sql, [key]).map(drop)
}

/// `2026-10-01 03:43:01.123Z`.
pub fn utc_stamp(t: SystemTime) -> String {
    chrono::DateTime::<chrono::Utc>::from(t).format("%Y-%m-%d %H:%M:%S%.3fZ").to_string()
}

#[cfg(test)]
pub fn memory() -> Connection {
    let db = Connection::open_in_memory().unwrap();
    init(&db).unwrap();
    db
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::{Entry, History};
    use std::time::UNIX_EPOCH;

    fn entry(host: &str, login: &str) -> Entry {
        Entry { host: host.into(), port: "5000".into(), login: login.into() }
    }

    #[test]
    fn history_round_trips_in_order_per_server() {
        let db = memory();
        let mut h = History::default();
        for (host, login) in [("a.net", "ann"), ("b.net", "bea"), ("a.net", "cid"), ("a.net", "ann")] {
            h.record(entry(host, login));
        }
        h.save(&db).unwrap();
        h.save(&db).unwrap();
        let loaded = History::load(&db).unwrap();
        assert_eq!(loaded, h);
        assert_eq!(loaded.logins("a.net", "5000").map(|e| e.login.as_str()).collect::<Vec<_>>(), ["ann", "cid"]);
        assert_eq!(loaded.logins("b.net", "5000").map(|e| e.login.as_str()).collect::<Vec<_>>(), ["bea"]);
    }

    #[test]
    fn owners_opening_a_new_database_at_once_all_get_a_connection() {
        let dir = std::env::temp_dir().join(format!("ggs-store-concurrent-{}", std::process::id()));
        for round in 0..20 {
            let path = dir.join(format!("{round}.db"));
            std::fs::create_dir_all(&dir).unwrap();
            let start = std::sync::Arc::new(std::sync::Barrier::new(8));
            let opened: Vec<_> = (0..8)
                .map(|_| {
                    let (path, start) = (path.clone(), start.clone());
                    std::thread::spawn(move || {
                        start.wait();
                        open_at(&path).map(drop)
                    })
                })
                .collect();
            for result in opened {
                result.join().unwrap().unwrap_or_else(|e| panic!("round {round}: {e}"));
            }
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn stamps_are_utc_dates_with_milliseconds() {
        assert_eq!(utc_stamp(UNIX_EPOCH), "1970-01-01 00:00:00.000Z");
        assert_eq!(utc_stamp(UNIX_EPOCH + Duration::from_millis(1_790_826_181_123)), "2026-10-01 03:43:01.123Z");
        assert_eq!(utc_stamp(UNIX_EPOCH + Duration::from_secs(951_782_400)), "2000-02-29 00:00:00.000Z", "leap day");
    }
}
