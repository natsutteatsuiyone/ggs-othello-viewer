use crate::store::utc_stamp;
use rusqlite::Connection;
use std::sync::{
    Arc, Mutex,
    mpsc::{self, SyncSender, TrySendError},
};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime};

/// Where a chat line belongs: a channel such as `.chat`, a private conversation with one user, or the
/// kibitz of one game.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum ChatKey {
    Channel(String),
    User(String),
    Game(String),
}

impl ChatKey {
    /// The name shown on the conversation and in the chat log.
    pub fn label(&self) -> String {
        match self {
            ChatKey::Channel(c) => c.clone(),
            ChatKey::User(u) => format!("tell {u}"),
            ChatKey::Game(id) => format!("kibitz {id}"),
        }
    }
}

#[derive(Clone, PartialEq, Debug)]
pub struct ChatLine {
    pub from: String,
    /// A multi-line message keeps its lines.
    pub lines: Vec<String>,
}

/// Lines that arrived while another conversation was open; `ForMe` when one of them is a tell or
/// names the login.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Unread {
    No,
    Yes,
    ForMe,
}

/// A line of a conversation and when it was added.
#[derive(Clone, PartialEq, Debug)]
pub struct Said {
    pub at: SystemTime,
    pub line: ChatLine,
}

#[derive(Debug)]
pub struct Conversation {
    pub key: ChatKey,
    pub lines: Vec<Said>,
    pub unread: Unread,
}

/// Channel and private conversations of one login. Every line, kibitz included, is also added to the
/// chat table with a timestamp.
#[derive(Debug)]
pub struct Chat {
    pub conversations: Vec<Conversation>,
    pub selected: ChatKey,
    /// A bounded writer owns the chat connection; no database operation runs in `add`.
    log: Option<ChatLog>,
    /// The login, whose own lines never ask for attention.
    me: String,
}

/// `.chat`, the channel every login is in, is open before anything arrives.
pub const DEFAULT_CHANNEL: &str = ".chat";

impl Chat {
    pub fn new(log: Option<(Connection, String)>, me: String) -> Chat {
        let key = ChatKey::Channel(DEFAULT_CHANNEL.into());
        let log = log.map(|(db, server)| ChatLog::new(db, server, me.clone(), LOG_QUEUE_CAPACITY));
        let mut chat = Chat { conversations: Vec::new(), selected: key.clone(), log, me };
        chat.conversation(&key);
        chat
    }

    fn conversation(&mut self, key: &ChatKey) -> &mut Conversation {
        let i = self.conversations.iter().position(|c| c.key == *key).unwrap_or_else(|| {
            self.conversations.push(Conversation { key: key.clone(), lines: Vec::new(), unread: Unread::No });
            self.conversations.len() - 1
        });
        &mut self.conversations[i]
    }

    pub fn me(&self) -> &str {
        &self.me
    }

    pub fn open(&mut self, key: ChatKey) {
        self.conversation(&key).unread = Unread::No;
        self.selected = key;
    }

    /// Closes a private conversation locally; a later tell opens it again.
    pub fn close(&mut self, key: &ChatKey) {
        if !matches!(key, ChatKey::User(_)) {
            return;
        }
        self.conversations.retain(|c| &c.key != key);
        if &self.selected == key {
            self.open(ChatKey::Channel(DEFAULT_CHANNEL.into()));
        }
    }

    /// Follows a `chann + .a .b` or `chann - .a` the user sends: a joined channel gets its conversation
    /// before anyone speaks there, and a left one is closed.
    pub fn follow_chann(&mut self, line: &str) {
        for line in line.split(['\r', '\n']) {
            self.follow_chann_line(line);
        }
    }

    fn follow_chann_line(&mut self, line: &str) {
        let mut tokens = crate::net::command_words(line);
        if tokens.next() != Some("chann") {
            return;
        }
        let join = match tokens.next() {
            Some("+") => true,
            Some("-") => false,
            _ => return,
        };
        for key in tokens.filter(|t| t.starts_with('.')).map(|t| ChatKey::Channel(t.into())) {
            if join {
                self.conversation(&key);
            } else {
                self.conversations.retain(|c| c.key != key);
                if self.selected == key {
                    self.open(ChatKey::Channel(DEFAULT_CHANNEL.into()));
                }
            }
        }
    }

    pub fn current(&self) -> Option<&Conversation> {
        self.conversations.iter().find(|c| c.key == self.selected)
    }

    pub fn unread(&self) -> Unread {
        self.conversations.iter().map(|c| c.unread).max().unwrap_or(Unread::No)
    }

    /// A line for a conversation, received or sent; kibitz goes only to the log, since the game shows it.
    /// True when the line is for the user: a tell, or a channel line that names the login.
    pub fn add(&mut self, key: ChatKey, line: ChatLine, now: SystemTime) -> bool {
        self.write_log(&key, &line, now);
        let for_me = match &key {
            ChatKey::Game(_) => return false,
            ChatKey::User(_) => line.from != self.me,
            ChatKey::Channel(_) => line.from != self.me && line.lines.iter().any(|t| mentions(t, &self.me)),
        };
        let unread = match (self.selected == key, for_me) {
            (true, _) => Unread::No,
            (false, false) => Unread::Yes,
            (false, true) => Unread::ForMe,
        };
        let c = self.conversation(&key);
        c.unread = c.unread.max(unread);
        c.lines.push(Said { at: now, line });
        for_me
    }

    fn write_log(&self, key: &ChatKey, line: &ChatLine, now: SystemTime) {
        if let Some(log) = &self.log {
            log.enqueue(LogEntry {
                at: utc_stamp(now),
                conversation: key.label(),
                sender: line.from.clone(),
                lines: line.lines.clone(),
            });
        }
    }

    /// Polls failures without waiting for the writer; repeated failures are summarized at most
    /// once per ten seconds, while the chat itself remains visible in memory.
    pub fn take_log_warning(&mut self, now: Instant) -> Option<String> {
        self.log.as_mut()?.take_warning(now)
    }
}

/// Bound whole messages, including their multi-line text, rather than building an unlimited backlog.
const LOG_QUEUE_CAPACITY: usize = 128;
const LOG_WARNING_INTERVAL: Duration = Duration::from_secs(10);

#[derive(Debug)]
struct LogEntry {
    at: String,
    conversation: String,
    sender: String,
    lines: Vec<String>,
}

#[derive(Debug, Default)]
struct LogFailures {
    pending: Mutex<Option<(usize, String)>>,
}

impl LogFailures {
    fn record(&self, lost_lines: usize, error: impl AsRef<str>) {
        // Construct the bounded detail before taking the lock; this lock never covers database work.
        let error: String = error.as_ref().chars().take(240).collect();
        let mut pending = self.pending.lock().unwrap_or_else(|e| e.into_inner());
        let count = pending.as_ref().map_or(0, |(count, _)| *count).saturating_add(lost_lines);
        *pending = Some((count, error));
    }
}

#[derive(Debug)]
struct ChatLog {
    queue: Option<SyncSender<LogEntry>>,
    failures: Arc<LogFailures>,
    worker: Option<JoinHandle<Connection>>,
    last_warning: Option<Instant>,
}

impl ChatLog {
    fn new(mut db: Connection, server: String, login: String, capacity: usize) -> Self {
        let (queue, incoming) = mpsc::sync_channel::<LogEntry>(capacity);
        let failures = Arc::new(LogFailures::default());
        let result = thread::Builder::new().name("ggs-chat-log".into()).spawn({
            let failures = failures.clone();
            move || {
                // SQLite's busy timeout from `store::open` waits here; dropping the UI never joins this thread.
                while let Ok(entry) = incoming.recv() {
                    if let Err(error) = persist_chat(&mut db, &server, &login, &entry) {
                        failures.record(entry.lines.len(), error.to_string());
                    }
                }
                db
            }
        });
        let worker = match result {
            Ok(worker) => Some(worker),
            Err(error) => {
                failures.record(0, format!("chat log writer could not start: {error}"));
                None
            }
        };
        Self { queue: worker.as_ref().map(|_| queue), failures, worker, last_warning: None }
    }

    fn enqueue(&self, entry: LogEntry) {
        if entry.lines.is_empty() {
            return;
        }
        let lost_lines = entry.lines.len();
        match self.queue.as_ref().map(|queue| queue.try_send(entry)) {
            Some(Ok(())) => {}
            Some(Err(TrySendError::Full(_))) => self.failures.record(lost_lines, "chat log queue is full"),
            Some(Err(TrySendError::Disconnected(_))) | None => {
                self.failures.record(lost_lines, "chat log writer is unavailable")
            }
        }
    }

    fn take_warning(&mut self, now: Instant) -> Option<String> {
        if self.last_warning.is_some_and(|at| now.saturating_duration_since(at) < LOG_WARNING_INTERVAL) {
            return None;
        }
        // A warning tick must not wait behind even the short failure-summary lock.
        let pending = self.failures.pending.try_lock().ok()?.take()?;
        self.last_warning = Some(now);
        Some(format!("chat log could not be saved ({} line(s) lost; last error: {})", pending.0, pending.1))
    }

    fn close(&mut self) {
        self.queue.take(); // The receiver drains accepted entries, then exits.
    }

    #[cfg(test)]
    fn finish(mut self) -> Connection {
        self.close();
        finish_worker(self.worker.take().expect("writer started"))
    }
}

impl Drop for ChatLog {
    fn drop(&mut self) {
        self.close();
        // Dropping JoinHandle detaches: a locked database cannot hold the UI open at shutdown.
        self.worker.take();
    }
}

fn persist_chat(db: &mut Connection, server: &str, login: &str, entry: &LogEntry) -> rusqlite::Result<()> {
    // One transaction makes a multi-line message all-or-nothing, and preserves enqueue order.
    let transaction = db.transaction()?;
    {
        let mut insert = transaction.prepare_cached(
            "INSERT INTO chat(server, login, at, conversation, sender, line) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        )?;
        for text in &entry.lines {
            insert.execute([server, login, &entry.at, &entry.conversation, &entry.sender, text])?;
        }
    }
    transaction.commit()
}

#[cfg(test)]
fn finish_worker(worker: JoinHandle<Connection>) -> Connection {
    // Never let a worker regression hang the test runner at join(). The helper thread can
    // wait for the join, while the test has a generous, real completion deadline.
    let (done, result) = mpsc::sync_channel(1);
    thread::spawn(move || {
        let _ = done.send(worker.join());
    });
    result
        .recv_timeout(Duration::from_secs(10))
        .expect("chat writer did not finish before the test deadline")
        .expect("writer completed")
}

/// `name` as a whole word of `text`, ignoring ASCII case.
fn mentions(text: &str, name: &str) -> bool {
    let word = |c: char| c.is_alphanumeric() || c == '_';
    !name.is_empty()
        && text.char_indices().any(|(i, _)| {
            text.get(i..i + name.len()).is_some_and(|w| w.eq_ignore_ascii_case(name))
                && !text[..i].chars().next_back().is_some_and(word)
                && !text[i + name.len()..].chars().next().is_some_and(word)
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, UNIX_EPOCH};

    fn line(from: &str, text: &str) -> ChatLine {
        ChatLine { from: from.into(), lines: vec![text.into()] }
    }

    #[test]
    fn chann_lines_open_and_close_conversations() {
        let mut chat = Chat::new(None, "me".into());
        let keys = |c: &Chat| c.conversations.iter().map(|c| c.key.label()).collect::<Vec<_>>();
        chat.follow_chann("chann + .tourney .yoka");
        assert_eq!(keys(&chat), [".chat", ".tourney", ".yoka"]);
        chat.open(ChatKey::Channel(".yoka".into()));
        chat.follow_chann("chann - .yoka");
        assert_eq!(keys(&chat), [".chat", ".tourney"]);
        assert_eq!(chat.selected, ChatKey::Channel(".chat".into()), "leaving the open channel goes back to .chat");
        chat.follow_chann("chann");
        chat.follow_chann("tell .tourney chann + .x");
        assert_eq!(keys(&chat), [".chat", ".tourney"]);
    }

    #[test]
    fn prefixed_channel_commands_follow_ggs_word_boundaries() {
        let mut chat = Chat::new(None, "me".into());
        let channel = ChatKey::Channel(".tourney".into());
        chat.follow_chann("\x0B: ;\tchann + .tourney");
        assert!(chat.conversations.iter().any(|c| c.key == channel));
        chat.open(channel.clone());
        chat.follow_chann(" ; : chann - .tourney");
        assert!(!chat.conversations.iter().any(|c| c.key == channel));
        assert_eq!(chat.selected, ChatKey::Channel(DEFAULT_CHANNEL.into()));
        for command in
            [":chann + .x", ";chann + .x", "alias + .x", "tell bob chann + .x", "\u{a0}chann + .x", "chann\u{3000}+ .x"]
        {
            chat.follow_chann(command);
        }
        assert_eq!(chat.conversations.len(), 1, "unknown aliases and non-command whitespace must not change channels");
        chat.follow_chann(": chann + .x\r\n; chann - .x");
        assert_eq!(chat.conversations.len(), 1, "each line is a separate command");
    }

    #[test]
    fn closing_a_tell_returns_to_chat_and_incoming_messages_reopen_it() {
        let mut chat = Chat::new(None, "me".into());
        let bob = ChatKey::User("bob".into());
        let eve = ChatKey::User("eve".into());
        chat.open(bob.clone());
        chat.open(eve.clone());
        chat.close(&bob);
        assert_eq!(chat.selected, eve, "closing a background conversation keeps the selection");
        chat.close(&eve);
        assert_eq!(chat.selected, ChatKey::Channel(DEFAULT_CHANNEL.into()));
        assert_eq!(chat.conversations.len(), 1);
        chat.close(&chat.selected.clone());
        assert_eq!(chat.conversations.len(), 1, "Close applies only to tells");
        chat.add(bob.clone(), line("bob", "hello again"), SystemTime::now());
        assert_eq!(chat.conversations.len(), 2);
        assert_eq!(chat.unread(), Unread::ForMe);
        chat.open(bob);
        assert_eq!(chat.current().unwrap().lines.len(), 1);
    }

    #[test]
    fn conversations_and_unread() {
        let mut chat = Chat::new(None, "me".into());
        let now = SystemTime::now();
        assert_eq!(
            chat.current().map(|c| &c.key),
            Some(&ChatKey::Channel(".chat".into())),
            ".chat is there from the start"
        );
        chat.add(ChatKey::Channel(".chat".into()), line("bob", "hi all"), now);
        assert_eq!(chat.unread(), Unread::No, "the open conversation is read");
        chat.add(ChatKey::User("bob".into()), line("bob", "hi you"), now);
        assert_eq!(chat.unread(), Unread::ForMe);
        chat.add(ChatKey::Game(".4".into()), line("bob", "nice move"), now);
        assert_eq!(chat.conversations.len(), 2, "kibitz is not a conversation");
        chat.open(ChatKey::User("bob".into()));
        assert_eq!(chat.unread(), Unread::No);
        assert_eq!(
            chat.current().unwrap().lines,
            [Said { at: now, line: line("bob", "hi you") }],
            "kept with its time"
        );
    }

    #[test]
    fn tells_and_mentions_are_for_me() {
        let mut chat = Chat::new(None, "Bob".into());
        let now = SystemTime::now();
        let chan = || ChatKey::Channel(".tourney".into());
        assert!(chat.add(ChatKey::User("eve".into()), line("eve", "psst"), now), "a tell");
        assert!(!chat.add(ChatKey::User("eve".into()), line("Bob", "hi eve"), now), "my own tell");
        assert!(chat.add(chan(), line("eve", "where is BOB?"), now), "a mention, any case");
        assert!(!chat.add(chan(), line("eve", "bob_"), now), "_ is part of a word");
        assert!(!chat.add(chan(), line("eve", "bobby is here"), now), "bobby is not bob");
        assert!(!chat.add(chan(), line("eve", "ask jimbob"), now), "jimbob is not bob");
        assert!(chat.add(chan(), line("eve", "bob"), now), "the whole line");
        assert!(chat.add(chan(), line("eve", "hi,bob."), now), "punctuation bounds a word");
        assert!(
            chat.add(chan(), ChatLine { from: "eve".into(), lines: vec!["first".into(), "ok bob".into()] }, now),
            "any line of a message"
        );
        assert!(!chat.add(chan(), line("Bob", "I am bob"), now), "my own line");
        assert!(!chat.add(ChatKey::Game(".4".into()), line("eve", "bob!"), now), "kibitz");
        assert!(!chat.add(ChatKey::Game(".4".into()), line("Bob", "own kibitz"), now), "own kibitz");
        assert!(
            chat.conversations.iter().all(|c| !matches!(c.key, ChatKey::Game(_))),
            "kibitz is never a conversation"
        );
        assert!(chat.add(ChatKey::Channel(DEFAULT_CHANNEL.into()), line("eve", "bob?"), now), "for me even while open");
        assert_eq!(chat.conversations[0].unread, Unread::No, "the open conversation is read");
    }

    #[test]
    fn unread_rises_to_for_me_and_opening_clears_it() {
        let mut chat = Chat::new(None, "bob".into());
        let now = SystemTime::now();
        let chan = || ChatKey::Channel(".tourney".into());
        chat.add(chan(), line("eve", "hello"), now);
        assert_eq!(chat.unread(), Unread::Yes);
        chat.add(chan(), line("eve", "bob, you there?"), now);
        assert_eq!(chat.unread(), Unread::ForMe);
        chat.add(chan(), line("eve", "anyone"), now);
        assert_eq!(chat.unread(), Unread::ForMe, "a plain line does not lower it");
        chat.open(chan());
        assert_eq!(chat.unread(), Unread::No);
    }

    #[test]
    fn every_line_reaches_the_log_under_its_server() {
        let at = UNIX_EPOCH + Duration::from_secs(1_790_826_181);
        let mut chat = Chat::new(Some((crate::store::memory(), "a.net:5000".into())), "pchat1".into());
        chat.add(ChatKey::Channel(".chat".into()), line("pchat2", "reply from b"), at);
        chat.add(
            ChatKey::User("pchat2".into()),
            ChatLine { from: "pchat2".into(), lines: vec!["two".into(), "lines".into()] },
            at,
        );
        chat.add(ChatKey::Game(".1".into()), line("pchat1", "kibitz to all"), at);
        let db = chat.log.take().unwrap().finish();
        let mut other = Chat::new(Some((db, "b.net:5000".into())), "pchat9".into());
        other.add(ChatKey::Channel(".chat".into()), line("eve", "elsewhere"), at);
        let db = other.log.take().unwrap().finish();
        let rows = |server: &str| {
            let mut q = db
                .prepare("SELECT at, login, conversation, sender, line FROM chat WHERE server = ?1 ORDER BY id")
                .unwrap();
            q.query_map([server], |r| Ok([r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?]))
                .unwrap()
                .map(Result::unwrap)
                .collect::<Vec<[String; 5]>>()
        };
        let stamp = "2026-10-01 03:43:01.000Z";
        assert_eq!(
            rows("a.net:5000"),
            [
                [stamp, "pchat1", ".chat", "pchat2", "reply from b"],
                [stamp, "pchat1", "tell pchat2", "pchat2", "two"],
                [stamp, "pchat1", "tell pchat2", "pchat2", "lines"],
                [stamp, "pchat1", "kibitz .1", "pchat1", "kibitz to all"],
            ]
        );
        assert_eq!(rows("b.net:5000"), [[stamp, "pchat9", ".chat", "eve", "elsewhere"]]);
    }

    struct LockedLog {
        path: std::path::PathBuf,
        owner: Option<Connection>,
    }

    impl LockedLog {
        fn new() -> (Self, Connection) {
            let unique = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
            let path = std::env::temp_dir().join(format!("ggs-chat-lock-{}-{unique}.db", std::process::id()));
            let owner = Connection::open(&path).unwrap();
            crate::store::init(&owner).unwrap();
            let writer = Connection::open(&path).unwrap();
            writer.busy_timeout(Duration::from_secs(5)).unwrap();
            owner.execute_batch("BEGIN IMMEDIATE").unwrap();
            (Self { path, owner: Some(owner) }, writer)
        }
    }

    impl Drop for LockedLog {
        fn drop(&mut self) {
            self.owner.take();
            let _ = std::fs::remove_file(&self.path);
        }
    }

    #[test]
    fn a_locked_database_does_not_block_chat_and_queue_overflow_is_reported() {
        let (locked, db) = LockedLog::new();
        let mut chat = Chat::new(None, "me".into());
        chat.log = Some(ChatLog::new(db, "a.net:5000".into(), "me".into(), 2));
        let started = Instant::now();
        // At most one message is in flight and two can be queued while SQLite remains locked.
        for i in 0..4 {
            chat.add(ChatKey::Channel(".chat".into()), line("bob", &format!("line {i}")), SystemTime::now());
        }
        assert!(started.elapsed() < Duration::from_secs(2), "add must not wait for SQLite's five-second busy timeout");
        assert_eq!(chat.current().unwrap().lines.len(), 4, "an unsaved line still appears in the chat");
        let warning = chat.take_log_warning(Instant::now()).expect("bounded queue overflow is visible");
        assert!(warning.contains("line(s) lost") && warning.contains("queue is full"), "{warning}");
        locked.owner.as_ref().unwrap().execute_batch("ROLLBACK").unwrap();
        let saved = chat.log.take().unwrap().finish();
        let count: i64 = saved.query_row("SELECT COUNT(*) FROM chat", [], |row| row.get(0)).unwrap();
        assert!((1..=3).contains(&count), "accepted entries drain after the lock is released; the rest were reported");
    }

    #[test]
    fn a_message_is_atomic_and_database_failures_are_reported_with_a_throttle() {
        let db = crate::store::memory();
        db.execute_batch("CREATE TRIGGER chat_fail BEFORE INSERT ON chat WHEN NEW.line = 'bad' BEGIN SELECT RAISE(ABORT, 'test save failure'); END;").unwrap();
        let mut chat = Chat::new(Some((db, "a.net:5000".into())), "me".into());
        chat.add(
            ChatKey::User("bob".into()),
            ChatLine { from: "bob".into(), lines: vec!["first".into(), "bad".into(), "last".into()] },
            SystemTime::now(),
        );
        chat.add(ChatKey::User("bob".into()), line("bob", "saved next"), SystemTime::now());
        let mut log = chat.log.take().unwrap();
        log.close();
        let db = finish_worker(log.worker.take().unwrap());
        let rows = db
            .prepare("SELECT line FROM chat ORDER BY id")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .map(Result::unwrap)
            .collect::<Vec<_>>();
        assert_eq!(
            rows,
            ["saved next"],
            "a failed multi-line transaction leaves no partial message, and the worker continues"
        );
        chat.log = Some(log);
        let now = Instant::now();
        let warning = chat.take_log_warning(now).unwrap();
        assert!(warning.contains("3 line(s) lost") && warning.contains("test save failure"), "{warning}");
        let failures = &chat.log.as_ref().unwrap().failures;
        failures.record(2, "first repeated failure");
        assert!(chat.take_log_warning(now + Duration::from_secs(1)).is_none());
        chat.log.as_ref().unwrap().failures.record(4, "last repeated failure");
        let warning = chat.take_log_warning(now + LOG_WARNING_INTERVAL).unwrap();
        assert!(warning.contains("6 line(s) lost") && warning.contains("last repeated failure"), "{warning}");
        assert!(chat.take_log_warning(now + LOG_WARNING_INTERVAL * 2).is_none(), "reported failures are consumed");
    }

    #[test]
    fn dropping_chat_drains_accepted_messages_in_order_without_joining_the_ui() {
        let mut chat = Chat::new(Some((crate::store::memory(), "a.net:5000".into())), "me".into());
        for i in 0..32 {
            chat.add(ChatKey::Channel(".chat".into()), line("bob", &format!("line {i}")), UNIX_EPOCH);
        }
        let worker = chat.log.as_mut().unwrap().worker.take().unwrap();
        let started = Instant::now();
        drop(chat);
        assert!(started.elapsed() < Duration::from_secs(2), "drop does not join the writer");
        let db = finish_worker(worker);
        let rows = db
            .prepare("SELECT line FROM chat ORDER BY id")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .map(Result::unwrap)
            .collect::<Vec<_>>();
        assert_eq!(rows, (0..32).map(|i| format!("line {i}")).collect::<Vec<_>>());
    }
}
