//! The servers and logins the login screen offers again: one entry per server and login, latest first.

use rusqlite::Connection;

const CAP: usize = 50;

#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub host: String,
    pub port: String,
    pub login: String,
}

#[derive(Debug, Default, PartialEq)]
pub struct History {
    entries: Vec<Entry>,
}

impl History {
    pub fn load(db: &Connection) -> rusqlite::Result<History> {
        let mut rows = db.prepare("SELECT host, port, login FROM logins ORDER BY rank")?;
        let entries = rows
            .query_map([], |r| Ok(Entry { host: r.get(0)?, port: r.get(1)?, login: r.get(2)? }))?
            .collect::<rusqlite::Result<_>>()?;
        Ok(History { entries })
    }

    /// Replaces the stored list; it is at most [`CAP`] rows.
    pub fn save(&self, db: &Connection) -> rusqlite::Result<()> {
        let tx = db.unchecked_transaction()?;
        tx.execute("DELETE FROM logins", [])?;
        for (rank, e) in self.entries.iter().enumerate() {
            tx.execute(
                "INSERT INTO logins(host, port, login, rank) VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![e.host, e.port, e.login, rank as i64],
            )?;
        }
        tx.commit()
    }

    /// Puts a connection first.
    pub fn record(&mut self, entry: Entry) {
        self.entries.retain(|e| *e != entry);
        self.entries.insert(0, entry);
        self.entries.truncate(CAP);
    }

    pub fn latest(&self) -> Option<&Entry> {
        self.entries.first()
    }

    /// Each server once, as its latest entry.
    pub fn servers(&self) -> Vec<&Entry> {
        let mut seen: Vec<&Entry> = Vec::new();
        for e in &self.entries {
            if !seen.iter().any(|s| (&s.host, &s.port) == (&e.host, &e.port)) {
                seen.push(e);
            }
        }
        seen
    }

    pub fn logins<'a>(&'a self, host: &'a str, port: &'a str) -> impl Iterator<Item = &'a Entry> {
        self.entries.iter().filter(move |e| e.host == host && e.port == port)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(host: &str, login: &str) -> Entry {
        Entry { host: host.into(), port: "5000".into(), login: login.into() }
    }

    #[test]
    fn logins_are_kept_per_server_latest_first() {
        let mut h = History::default();
        for (host, login) in [("a.net", "ann"), ("b.net", "bea"), ("a.net", "cid"), ("a.net", "ann")] {
            h.record(entry(host, login));
        }
        let names = |host| h.logins(host, "5000").map(|e| e.login.as_str()).collect::<Vec<_>>();
        assert_eq!(names("a.net"), ["ann", "cid"]);
        assert_eq!(names("b.net"), ["bea"]);
        assert_eq!(h.servers().iter().map(|e| e.host.as_str()).collect::<Vec<_>>(), ["a.net", "b.net"]);
    }
}
