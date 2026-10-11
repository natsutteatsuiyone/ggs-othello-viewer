//! `ggs.db`: logins, settings and chat lines. Chat rows carry their server as `host:port`.

use crate::history::History;
use rusqlite::Connection;
use std::path::Path;
use std::time::{Duration, Instant, SystemTime};

const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// Files of earlier versions, imported once and then deleted.
const LOGIN_FILE: &str = "ggs-login.txt";
const THEME_FILE: &str = "ggs-theme.txt";

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
    import_legacy(&db, path.parent().unwrap_or(Path::new("")))?;
    // Versions before the data folder kept these files in the working directory.
    #[cfg(not(test))]
    import_legacy(&db, Path::new(""))?;
    Ok(db)
}

pub fn init(db: &Connection) -> rusqlite::Result<()> {
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS logins(host TEXT NOT NULL, port TEXT NOT NULL, login TEXT NOT NULL, rank INTEGER NOT NULL, PRIMARY KEY(host, port, login));
         CREATE TABLE IF NOT EXISTS saved_passwords(key TEXT PRIMARY KEY);
         CREATE TABLE IF NOT EXISTS settings(key TEXT PRIMARY KEY, value TEXT NOT NULL);
         CREATE TABLE IF NOT EXISTS chat(id INTEGER PRIMARY KEY, server TEXT NOT NULL, login TEXT NOT NULL, at TEXT NOT NULL, conversation TEXT NOT NULL, sender TEXT NOT NULL, line TEXT NOT NULL);
         CREATE INDEX IF NOT EXISTS chat_server ON chat(server, id);",
    )?;
    // Earlier versions kept a protocol trace that nothing read.
    db.execute_batch("DROP TABLE IF EXISTS raw")?;
    // Earlier versions kept lines to send after login; the app no longer sends typed commands.
    let has_commands: bool =
        db.query_row("SELECT EXISTS (SELECT 1 FROM pragma_table_info('logins') WHERE name = 'commands')", [], |r| {
            r.get(0)
        })?;
    if has_commands {
        db.execute_batch("ALTER TABLE logins DROP COLUMN commands")?;
    }
    Ok(())
}

/// Moves `ggs-login.txt` and `ggs-theme.txt` from `dir` into the db. Logins and theme already in the db win.
pub fn import_legacy(db: &Connection, dir: &Path) -> rusqlite::Result<()> {
    let login = dir.join(LOGIN_FILE);
    if let Ok(text) = std::fs::read_to_string(&login) {
        if db.query_row("SELECT NOT EXISTS (SELECT 1 FROM logins)", [], |r| r.get(0))? {
            History::parse(&text).save(db)?;
        }
        let _ = std::fs::remove_file(&login);
    }
    let theme = dir.join(THEME_FILE);
    if let Ok(text) = std::fs::read_to_string(&theme) {
        db.execute("INSERT OR IGNORE INTO settings(key, value) VALUES ('theme', ?1)", [text.trim()])?;
        let _ = std::fs::remove_file(&theme);
    }
    Ok(())
}

/// `2026-10-01 03:43:01.123Z`.
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
    use crate::history::Entry;
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
    fn legacy_login_file_is_imported_once_and_removed() {
        let dir = std::env::temp_dir().join(format!("ggs-store-import-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(LOGIN_FILE), "a.net\t5000\tann\twho\nb.net\t5000\tbea\t\n").unwrap();
        std::fs::write(dir.join(THEME_FILE), "Dark\n").unwrap();
        let db = memory();
        import_legacy(&db, &dir).unwrap();
        assert!(!dir.join(LOGIN_FILE).exists() && !dir.join(THEME_FILE).exists());
        std::fs::write(dir.join(LOGIN_FILE), "c.net\t5000\tcid\t\n").unwrap();
        import_legacy(&db, &dir).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        let h = History::load(&db).unwrap();
        assert_eq!(
            h.servers().iter().map(|e| e.host.as_str()).collect::<Vec<_>>(),
            ["a.net", "b.net"],
            "a second file does not replace imported logins"
        );
        assert_eq!(
            db.query_row("SELECT value FROM settings WHERE key = 'theme'", [], |r| r.get::<_, String>(0)).unwrap(),
            "Dark"
        );
    }

    #[test]
    fn the_commands_column_of_earlier_versions_is_dropped() {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch("CREATE TABLE logins(host TEXT NOT NULL, port TEXT NOT NULL, login TEXT NOT NULL, commands TEXT NOT NULL, rank INTEGER NOT NULL, PRIMARY KEY(host, port, login));
             INSERT INTO logins VALUES ('a.net', '5000', 'ann', 'tell /os play', 0);").unwrap();
        init(&db).unwrap();
        init(&db).unwrap();
        assert!(db.prepare("SELECT commands FROM logins").is_err(), "the stored commands are gone");
        assert_eq!(
            History::load(&db).unwrap().latest().map(|e| e.login.as_str()),
            Some("ann"),
            "the login itself is kept"
        );
    }

    #[test]
    fn the_raw_table_of_earlier_versions_is_dropped() {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch("CREATE TABLE raw(id INTEGER PRIMARY KEY, line TEXT NOT NULL)").unwrap();
        init(&db).unwrap();
        assert!(db.prepare("SELECT line FROM raw").is_err());
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
