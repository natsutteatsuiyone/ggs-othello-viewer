use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::time::{Duration, Instant};

use crate::game::{Game, GameKey, GameSource, Phase, archive_id};
use crate::ggf::GameRecord;
use crate::net;
use crate::proto::{
    self, Archived, Finished, MatchEntry, MatchSettings, OS, OsMsg, Outcome, Profile, Rank, RatingType, ReplyKind,
    User, UserStatus,
};

/// How many ended matches the match list keeps, enough that a short series does not drop out.
const FINISHED_MAX: usize = 20;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum ViewMode {
    #[default]
    Focus,
    Synchro,
}

#[derive(Debug)]
pub struct FinishedMatch {
    pub report: Finished,
    boards: Vec<(String, GameKey)>,
}

impl FinishedMatch {
    pub fn is_held(&self, lobby: &Lobby) -> bool {
        let expected = if Game::is_synchro_type(&self.report.summary.game_type) { 2 } else { 1 };
        self.boards.len() == expected
            && self.boards.iter().all(|(id, key)| lobby.games.get(id).is_some_and(|g| g.key == *key))
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CommandSource {
    App,
    Console,
}

/// A command prepared by the UI. Preparing one changes no subscription or pending request;
/// `Lobby::send_app` commits its state and send order only after the connection queue accepts the line.
#[derive(Debug)]
pub enum AppCommand {
    Watch(String),
    /// A new connection needs the subscriptions whose intent survived the old one.
    RestoreWatch(String),
    Look(String),
    Unwatch(String),
    Who(RatingType),
    Top(RatingType),
    /// Stop receiving updates without throwing away the browsable record.
    RetainStop(String),
    /// A kibitz line to all who see `id`, or to its observers only; `send_app` also shows it here, since the server
    /// does not send it back to its sender.
    Kibitz {
        id: String,
        me: String,
        text: String,
        observers_only: bool,
    },
    Line(String),
}

impl From<String> for AppCommand {
    fn from(line: String) -> Self {
        Self::Line(line)
    }
}

impl AppCommand {
    fn line(&self) -> String {
        match self {
            Self::Watch(id) | Self::RestoreWatch(id) => proto::tell(&format!("watch + {id}")),
            Self::Look(key) => proto::tell(&format!("look {key}")),
            // `a` lists every user, not only those open for a match.
            Self::Who(t) => proto::tell(&format!("who {} a", t.key())),
            Self::Top(t) => proto::tell(&format!("top {} {}", t.key(), proto::GUI_TOP_COUNT)),
            Self::Unwatch(id) | Self::RetainStop(id) => proto::tell(&format!("watch - {id}")),
            Self::Kibitz { id, text, observers_only, .. } => {
                let to = if *observers_only { "o " } else { "" };
                proto::tell(&format!("tell {to}{id} {text}"))
            }
            Self::Line(line) => line.clone(),
        }
    }
}

#[derive(Debug)]
struct ListRequest {
    /// The send order of the line that asked.
    seq: u64,
    cache: Option<RatingType>,
    source: CommandSource,
    at: Instant,
}

#[derive(Debug)]
struct Sent {
    /// The send order.
    seq: u64,
    line: String,
    source: CommandSource,
}

/// Whether `/os` is on the server, as far as this login has seen.
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub enum OsState {
    #[default]
    Unseen,
    Present,
    /// The connection or /os was lost: the server holds no watches until /os announces itself again.
    Lost,
}

/// Everything the viewer knows about the server's `/os` state during one login.
#[derive(Default, Debug)]
pub struct Lobby {
    pub os: OsState,
    /// Lines sent on this connection whose ack has not arrived, oldest first.
    unacked: VecDeque<Sent>,
    /// Set when `unacked` overflowed: acks do not line up with it until one is its oldest line's.
    unacked_lost: bool,
    next_seq: u64,
    /// The send order of the latest watch line about each match, so an older line acked later
    /// does not undo it.
    watch_seq: BTreeMap<String, u64>,
    /// The logged-in login; the server keeps sending its own games whatever `watch` says.
    pub me: String,
    pub matches: Vec<MatchEntry>,
    /// The latest ended matches, oldest first.
    pub finished: VecDeque<FinishedMatch>,
    /// Match ids (`.N`) the server is sending us, in the order watched; a synchro match's boards are `.N.0` and `.N.1`.
    pub watched: Vec<String>,
    /// Watch notices precede join and belong to both boards of a synchro match.
    match_observers: BTreeMap<String, BTreeSet<String>>,
    pub view: ViewMode,
    /// Matches the user stopped watching; late joins and observer notices are ignored, except for the
    /// user's own matches, which stay closed while the server keeps sending them.
    dismissed: BTreeSet<String>,
    /// Games kept after their watch stopped; their records need a new join or an end to confirm them.
    stale: BTreeSet<String>,
    /// Game id shown in detail, or a match id while its first board is on the way.
    pub focus: Option<String>,
    /// Matches watched for a view other than the board, such as a tournament's live tiles; their joins never take the focus.
    pub background: BTreeSet<String>,
    /// The rating type the Users tab shows.
    pub users_type: RatingType,
    /// Online Othello users per rating type, open ones first.
    users_cache: BTreeMap<&'static str, Vec<User>>,
    pub ranks_type: RatingType,
    /// The `top` list per rating type.
    ranks_cache: BTreeMap<&'static str, Vec<Rank>>,
    /// Pending `who` and `top` requests, oldest first: replies name neither their pool nor their source.
    who_asked: VecDeque<ListRequest>,
    top_asked: VecDeque<ListRequest>,
    /// `/os finger` settings, which may arrive before `who`; `None` means an unknown service login.
    pub match_settings: BTreeMap<String, Option<MatchSettings>>,
    pub games: BTreeMap<String, Game>,
    /// Previous games used to verify identity before restoring their views after reconnecting.
    pub restoring_games: BTreeMap<String, Game>,
    /// Archive keys asked for with `look`, oldest first; replies come in order and do not name the key.
    looks: VecDeque<(u64, String)>,
    reopen_archives: BTreeSet<String>,
    /// `history` and `stored` replies by login.
    pub history: BTreeMap<String, Vec<Archived>>,
    pub stored: BTreeMap<String, Vec<Archived>>,
    /// The server's `finger` of each login asked about; `None` when the server does not know it.
    pub profiles: BTreeMap<String, Option<Profile>>,
    pub tourneys: crate::td::Tourneys,
}

pub fn match_of(game_id: &str) -> &str {
    match game_id.match_indices('.').nth(1) {
        Some((i, _)) => &game_id[..i],
        None => game_id,
    }
}

/// Records a subscription confirmed by a Join or committed at the send boundary.
fn add_watch(watched: &mut Vec<String>, match_id: &str) {
    if watched.iter().any(|w| w == match_id) {
        return;
    }
    watched.push(match_id.to_string());
}

/// How long an unanswered `who`/`top` counts as on its way, matching the app's request cooldown.
const ASK_PENDING: Duration = Duration::from_secs(5);

fn pending(asked: &VecDeque<ListRequest>, t: RatingType, now: Instant) -> bool {
    asked.iter().any(|a| a.cache == Some(t) && now.saturating_duration_since(a.at) < ASK_PENDING)
}

/// The words after `tell /os` in a line, or `None` for any other line.
fn os_words(line: &str) -> Option<Vec<&str>> {
    let rest = line.trim_matches(is_space).strip_prefix("tell")?;
    if !rest.starts_with(is_space) {
        return None;
    }
    // `tell` ends the recipient at the first plain space (Central EXE_Client.C `tell`): `/os\tlook` names no one.
    let (to, message) = rest.trim_start_matches(is_space).split_once(' ')?;
    (to == OS).then(|| message.split(is_space).filter(|w| !w.is_empty()).collect())
}

/// GGS splits on C `isspace`; `split_whitespace` would also split on U+00A0 or U+3000.
fn is_space(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\x0B' | '\x0C' | '\r')
}

/// Whether the server runs `line` as `tell`, which it never looks up as an alias (EXE_Client.C `shell_expand`).
fn is_tell(line: &str) -> bool {
    line.trim_start_matches(is_space)
        .strip_prefix("tell")
        .is_some_and(|rest| rest.is_empty() || rest.starts_with(is_space))
}

fn insert_in_order<T>(queue: &mut VecDeque<T>, item: T, seq: impl Fn(&T) -> u64) {
    let i = queue.iter().position(|x| seq(x) > seq(&item)).unwrap_or(queue.len());
    queue.insert(i, item);
}

/// Pops the request a list reply answers: whether the log shows the reply (asked from the console,
/// or not asked at all) and the cache it fills.
fn answered(asked: &mut VecDeque<ListRequest>) -> (bool, Option<RatingType>) {
    let request = asked.pop_front();
    (request.as_ref().is_none_or(|r| r.source == CommandSource::Console), request.and_then(|r| r.cache))
}

impl Lobby {
    /// A transport or service restart invalidates subscriptions and replies, not saved records.
    pub fn suspend(&mut self) {
        self.os = OsState::Lost;
        self.matches.clear();
        self.match_observers.clear();
        self.users_cache.clear();
        self.ranks_cache.clear();
        self.who_asked.clear();
        self.top_asked.clear();
        self.match_settings.clear();
        self.profiles.clear();
        self.tourneys = Default::default();
        self.history.clear();
        self.stored.clear();
        self.reopen_archives.extend(self.looks.drain(..).map(|(_, key)| key));
        for (id, mut game) in std::mem::take(&mut self.games) {
            if game.source == GameSource::Live && game.takes_moves() {
                game.observers.clear();
                self.restoring_games.insert(id, game);
            } else {
                self.games.insert(id, game);
            }
        }
    }

    pub fn reconnect_commands(&self) -> Vec<AppCommand> {
        let mut commands = self.watched.iter().cloned().map(AppCommand::RestoreWatch).collect::<Vec<_>>();
        for key in &self.reopen_archives {
            commands.extend(self.request_look(key));
        }
        commands
    }

    /// The only send boundary for app commands. Duplicate plans are checked again here, so
    /// preparing or dropping a plan never creates a watch or consumes a reply. State represents
    /// lines accepted by the connection queue, not a guarantee of delivery or server acceptance.
    /// The UI dispatches replies after this call returns, so registration precedes their handling.
    pub fn send_app(&mut self, command: AppCommand, enqueue: impl FnOnce(&str) -> bool) -> bool {
        match &command {
            AppCommand::Watch(id) if self.is_watched(id) => {
                // Reopening a player's own closed game needs no server subscription.
                self.dismissed.remove(id);
                return false;
            }
            AppCommand::Look(key) if self.archive_loaded_or_pending(key) => return false,
            AppCommand::Unwatch(id) if self.owns_match(id) => {
                self.release_focus(id);
                self.dismissed.insert(id.clone());
                return false;
            }
            AppCommand::Unwatch(id) if !self.is_watched(id) => return false,
            AppCommand::RetainStop(id) if !self.is_watched(id) || self.owns_match(id) => return false,
            _ => {}
        }
        let line = command.line();
        if !enqueue(&line) {
            return false;
        }
        match command {
            AppCommand::Watch(id) | AppCommand::RestoreWatch(id) => {
                // Explicit UI intent survives /os being away.
                self.claim_watch(&id, self.next_seq);
                self.dismissed.remove(&id);
                add_watch(&mut self.watched, &id);
            }
            AppCommand::Who(t) => self.users_type = t,
            AppCommand::Top(t) => self.ranks_type = t,
            AppCommand::Look(key) => {
                self.reopen_archives.remove(&key);
            }
            AppCommand::Unwatch(id) => {
                self.claim_watch(&id, self.next_seq);
                self.forget(&id);
            }
            AppCommand::RetainStop(id) => {
                self.claim_watch(&id, self.next_seq);
                self.retain_stop(&id);
            }
            AppCommand::Kibitz { id, me, text, observers_only } => {
                let shown =
                    if observers_only { format!("{me} (to observers): {text}") } else { format!("{me}: {text}") };
                self.add_text(&id, vec![shown]);
            }
            _ => {}
        }
        self.sent(&line, CommandSource::App);
        true
    }

    /// Registers lines sent to the server; app commands reach this through `send_app`.
    /// Lines the server expands wait for their ack: only the server knows its aliases and variables.
    pub fn sent(&mut self, command: &str, source: CommandSource) {
        for line in command.lines().filter(|l| !l.trim().is_empty()) {
            let seq = self.next_seq;
            self.next_seq += 1;
            if !line.contains('$') {
                self.register(line, source, seq);
            }
            // The log masks a `passw` ack to this, so keeping the line would only keep the password.
            let line = if net::is_secret(line) { "passw ********".to_string() } else { line.to_string() };
            self.unacked.push_back(Sent { seq, line, source });
            // ponytail: bounded in case the user turns acks off with `verbose -ack`; a full queue drops
            // every pending `$` line. Track `verbose ±ack` and skip queueing while acks are off once a
            // `$` request goes missing in a session that turned them off.
            if self.unacked.len() > 1024 {
                self.unacked.clear();
                self.unacked_lost = true;
            }
        }
    }

    /// Takes an ack, `<command> => <expanded>` (Central EXE_Client.C `shell_execute`). Acks come in
    /// send order, and `<command>` is the line as sent unless an alias expanded it, which acks once
    /// per command it runs. So an ack is an unacked line's own, with only aliases sent before it, or
    /// one of the oldest line's alias commands. `tell` is never an alias, so it bounds the search.
    pub fn acked(&mut self, ack: &str) {
        let own = |sent: &Sent| match ack.strip_prefix(sent.line.as_str())? {
            "" => Some(ack),
            rest => rest.strip_prefix(" => "),
        };
        let end = self.unacked.iter().position(|sent| is_tell(&sent.line)).map_or(self.unacked.len(), |i| i + 1);
        let mine = self.unacked.iter().take(end).position(|sent| own(sent).is_some());
        if let Some(i) = mine {
            self.unacked_lost = false;
            let sent = self.unacked.drain(..=i).next_back().expect("position is in range");
            if sent.line.contains('$') {
                let ran = own(&sent).expect("matched above").to_string();
                self.register(&ran, sent.source, sent.seq);
            }
        } else if !self.unacked_lost
            && let Some(&Sent { seq, .. }) = self.unacked.front()
        {
            // An alias's command; both sides are the same text, which may itself hold ` => `.
            let half = ack.len().saturating_sub(4) / 2;
            let ran = match (ack.get(..half), ack.get(half..)) {
                (Some(left), Some(rest)) if rest.strip_prefix(" => ") == Some(left) => left,
                _ => ack.split_once(" => ").map_or(ack, |(_, ran)| ran),
            };
            self.register(ran, CommandSource::Console, seq);
        }
    }

    /// The transport closed: no ack is coming for what was sent on it.
    pub fn connection_lost(&mut self) {
        self.unacked.clear();
        self.unacked_lost = false;
        self.suspend();
    }

    /// Records the request a `tell /os` line makes, in the place its send order gives it.
    fn register(&mut self, line: &str, source: CommandSource, seq: u64) {
        let Some(words) = os_words(line) else { return };
        let [command, args @ ..] = words.as_slice() else { return };
        // Watch lines replayed at a reconnect login reach a server that holds no watches yet.
        let restoring = self.os == OsState::Lost;
        match (*command, args) {
            ("look", _) => insert_in_order(&mut self.looks, (seq, args.join(" ")), |(s, _)| *s),
            ("who" | "top", _) => {
                if let Some(rating) = proto::list_query_cache_type(command, args) {
                    let asked = if *command == "who" { &mut self.who_asked } else { &mut self.top_asked };
                    insert_in_order(asked, ListRequest { seq, cache: rating, source, at: Instant::now() }, |r| r.seq);
                }
            }
            _ if restoring => {}
            ("watch", ["+", ids @ ..]) => {
                for id in ids {
                    if id.starts_with('.') && self.claim_watch(match_of(id), seq) {
                        self.dismissed.remove(match_of(id));
                        add_watch(&mut self.watched, match_of(id));
                    }
                }
            }
            ("watch", ["-", ids @ ..]) => {
                let stopped: Vec<&str> =
                    ids.iter().map(|id| match_of(id)).filter(|m| self.claim_watch(m, seq)).collect();
                self.stop_watches(|m| stopped.contains(&m));
            }
            ("watch", ["%"]) => {
                let stopped: Vec<String> =
                    self.watched.clone().into_iter().filter(|m| self.claim_watch(m, seq)).collect();
                self.stop_watches(|m| stopped.iter().any(|s| s == m));
            }
            _ => {}
        }
    }

    /// False when a watch line sent after `seq` already decided `m`; an aliased line is applied at its ack.
    fn claim_watch(&mut self, m: &str, seq: u64) -> bool {
        if self.watch_seq.get(m).is_some_and(|&later| later > seq) {
            return false;
        }
        self.watch_seq.insert(m.to_string(), seq);
        true
    }

    /// Forgets the watched matches `stop` names, except the login's own: the server answers a stop
    /// for those with ERR and keeps sending them.
    fn stop_watches(&mut self, stop: impl Fn(&str) -> bool) {
        let stopped: Vec<String> = self.watched.iter().filter(|m| stop(m) && !self.owns_match(m)).cloned().collect();
        for m in stopped {
            self.forget(&m);
        }
    }

    /// Focuses `id` (a listed match or board) and prepares its watch if needed.
    pub fn watch(&mut self, id: &str) -> Vec<AppCommand> {
        self.show(id);
        self.request_watch(id)
    }

    pub(crate) fn request_watch(&self, id: &str) -> Vec<AppCommand> {
        if !id.starts_with('.') || self.is_watched(id) && !self.dismissed.contains(match_of(id)) {
            return Vec::new();
        }
        vec![AppCommand::Watch(match_of(id).to_string())]
    }

    /// Focuses `id` in the detail view without subscribing to it, for a game already held.
    pub fn show(&mut self, id: &str) {
        self.view = ViewMode::Focus;
        // An already watched synchro match will not send another join. Resolve its parent to a
        // received board now, but keep an explicit board request pending until that board arrives.
        let focus = if id == match_of(id) && !self.games.contains_key(id) {
            self.games.keys().find(|game_id| match_of(game_id) == id).map(String::as_str).unwrap_or(id)
        } else {
            id
        };
        self.focus_on(focus.to_string());
    }

    /// Shows archived match `key` (`.73`) and prepares a lookup unless it is loaded or pending.
    pub fn look(&mut self, key: &str) -> Vec<AppCommand> {
        let id = archive_id(key);
        self.show(&id);
        self.request_look(key)
    }

    pub(crate) fn archive_pending(&self, id: &str) -> bool {
        self.looks.iter().any(|(_, key)| archive_id(key) == id)
    }

    fn archive_loaded_or_pending(&self, key: &str) -> bool {
        let id = archive_id(key);
        let loaded = self.games.keys().any(|g| match_of(g) == id);
        loaded || self.looks.iter().any(|(_, k)| k == key)
    }

    pub(crate) fn request_look(&self, key: &str) -> Vec<AppCommand> {
        if self.archive_loaded_or_pending(key) {
            return Vec::new();
        }
        vec![AppCommand::Look(key.to_string())]
    }

    /// What a user's page asks for: profile, match settings and archived games.
    pub fn games_of(&self, login: &str, refresh: bool) -> Vec<AppCommand> {
        [
            (format!("finger {login}"), self.profiles.contains_key(login)),
            (proto::tell(&format!("finger {login}")), self.match_settings.contains_key(login)),
            (proto::tell(&format!("history {login}")), self.history.contains_key(login)),
            (proto::tell(&format!("stored {login}")), self.stored.contains_key(login)),
        ]
        .into_iter()
        .filter_map(|(cmd, loaded)| (refresh || !loaded).then_some(AppCommand::Line(cmd)))
        .collect()
    }

    /// Stops watching the match that `id` belongs to and forgets its games. The user's own match is
    /// only closed: the server sends players their match whatever `watch` says.
    pub fn unwatch(&mut self, id: &str) -> Vec<AppCommand> {
        let m = match_of(id);
        if self.owns_match(m) {
            self.release_focus(m);
            self.dismissed.insert(m.to_string());
            return Vec::new();
        }
        if self.is_watched(m) {
            vec![AppCommand::Unwatch(m.into())]
        } else {
            self.forget(m);
            Vec::new()
        }
    }

    /// Drops match `m` with its boards and subscription; true when it was watched.
    fn forget(&mut self, m: &str) -> bool {
        self.match_observers.remove(m);
        self.games.retain(|gid, _| match_of(gid) != m);
        self.restoring_games.retain(|gid, _| match_of(gid) != m);
        self.stale.retain(|gid| match_of(gid) != m);
        self.release_focus(m);
        self.dismissed.insert(m.to_string());
        let before = self.watched.len();
        self.watched.retain(|w| w != m);
        self.watched.len() != before
    }

    /// Clears a focus on `m` or one waiting for it, and drops out of the Synchro view if that was showing it.
    fn release_focus(&mut self, m: &str) {
        if self.focus.as_deref().is_some_and(|f| match_of(f) == m) {
            self.focus = None;
            if self.view == ViewMode::Synchro {
                self.view = ViewMode::Focus;
            }
        }
    }

    pub fn show_view(&mut self, view: ViewMode) -> Vec<AppCommand> {
        self.view = view;
        Vec::new()
    }

    /// Shows this type's list if it is loaded or on its way; false means it has to be asked for.
    pub fn show_users(&mut self, t: RatingType, now: Instant) -> bool {
        let shown = self.users_cache.contains_key(t.key()) || pending(&self.who_asked, t, now);
        if shown {
            self.users_type = t;
        }
        shown
    }

    pub fn show_ranks(&mut self, t: RatingType, now: Instant) -> bool {
        let shown = self.ranks_cache.contains_key(t.key()) || pending(&self.top_asked, t, now);
        if shown {
            self.ranks_type = t;
        }
        shown
    }

    /// The shown type's list is asked for and not here yet.
    pub fn users_loading(&self, now: Instant) -> bool {
        !self.users_cache.contains_key(self.users_type.key()) && pending(&self.who_asked, self.users_type, now)
    }

    pub fn ranks_loading(&self, now: Instant) -> bool {
        !self.ranks_cache.contains_key(self.ranks_type.key()) && pending(&self.top_asked, self.ranks_type, now)
    }

    pub fn users(&self) -> &[User] {
        self.users_cache.get(self.users_type.key()).map_or(&[], Vec::as_slice)
    }

    pub fn ranks(&self) -> &[Rank] {
        self.ranks_cache.get(self.ranks_type.key()).map_or(&[], Vec::as_slice)
    }

    /// Kibitz reaches a game only while it is live and watched.
    pub fn can_kibitz(&self, g: &Game) -> bool {
        g.source == GameSource::Live && g.phase == Phase::Live && self.is_watched(&g.id)
    }

    pub fn is_watched(&self, id: &str) -> bool {
        self.watched.iter().any(|w| w == match_of(id))
    }

    fn is_player(&self, player1: &str, player2: &str) -> bool {
        !self.me.is_empty() && (player1 == self.me || player2 == self.me)
    }

    fn owns_match(&self, id: &str) -> bool {
        // The listing, else a board received on this connection: a parked record does not prove that
        // the id still belongs to this login after an outage.
        if let Some(m) = self.matches.iter().find_map(|e| match e {
            MatchEntry::Summary(m) if match_of(&m.id) == id => Some(m),
            _ => None,
        }) {
            return self.is_player(&m.player1, &m.player2);
        }
        self.games
            .values()
            .any(|g| match_of(&g.id) == id && g.phase == Phase::Live && self.is_player(&g.black.name, &g.white.name))
    }

    /// Prepares stops for unused subscriptions, keeping records and browsing state for reopening.
    /// At the send boundary observers are cleared: a new watch announces those present but not those
    /// who left in between. The server sends the login its own matches regardless of `watch`.
    pub fn retain_watches(&self, keep: impl Fn(&str) -> bool) -> Vec<AppCommand> {
        self.watched
            .iter()
            .filter(|id| !keep(id) && !self.owns_match(id))
            .cloned()
            .map(AppCommand::RetainStop)
            .collect()
    }

    /// A disconnected session has no transport queue, but still needs to drop abandoned intent
    /// before reconnecting. This is an explicit local action, separate from command preparation.
    pub fn discard_watches(&mut self, keep: impl Fn(&str) -> bool) {
        for command in self.retain_watches(keep) {
            if let AppCommand::RetainStop(id) = command {
                self.retain_stop(&id);
            }
        }
    }

    fn retain_stop(&mut self, id: &str) {
        self.match_observers.remove(id);
        self.watched.retain(|watched| watched != id);
        for g in self.games.values_mut().filter(|g| match_of(&g.id) == id) {
            g.observers.clear();
            if g.takes_moves() {
                self.stale.insert(g.id.clone());
            }
        }
        self.dismissed.insert(id.to_string());
    }

    /// The running match `login` plays in, as the match list shows it.
    pub fn playing(&self, login: &str) -> Option<&str> {
        self.matches.iter().find_map(|e| match e {
            MatchEntry::Summary(m) if m.player1 == login || m.player2 == login => Some(match_of(&m.id)),
            _ => None,
        })
    }

    /// The match list without synchro containers `.N` whose boards `.N.0`/`.N.1` are listed.
    pub fn visible_matches(&self) -> impl Iterator<Item = &MatchEntry> {
        let parents: BTreeSet<&str> =
            self.matches.iter().map(|e| e.id()).filter(|&id| match_of(id) != id).map(match_of).collect();
        self.matches.iter().filter(move |e| !parents.contains(e.id()))
    }

    /// The focused game and its synchro twin.
    pub fn halves(&self) -> impl Iterator<Item = &Game> {
        let m = self.focus.as_deref().map(match_of);
        self.games.values().filter(move |g| m == Some(match_of(&g.id)))
    }

    /// The selected synchro match's `.0` and `.1` boards. A slot stays empty while that board is arriving.
    pub fn synchro_halves(&self) -> [Option<&Game>; 2] {
        let Some(m) = self.focus.as_deref().map(match_of) else { return [None, None] };
        [self.games.get(&format!("{m}.0")), self.games.get(&format!("{m}.1"))]
    }

    /// Whether the selected match is known to be synchro, including while its boards are being restored.
    pub fn has_synchro_focus(&self) -> bool {
        if self.view == ViewMode::Synchro {
            return true;
        }
        let Some(focus) = self.focus.as_deref() else { return false };
        let m = match_of(focus);
        let is_half = |id: &str| id.strip_prefix(m).is_some_and(|suffix| suffix == ".0" || suffix == ".1");
        is_half(focus)
            || self
                .games
                .values()
                .chain(self.restoring_games.values())
                .any(|g| match_of(&g.id) == m && (is_half(&g.id) || g.is_synchro()))
            || self.matches.iter().any(|entry| match entry {
                MatchEntry::Summary(summary) => match_of(&summary.id) == m && Game::is_synchro_type(&summary.game_type),
                MatchEntry::Raw(_) => false,
            })
    }

    pub fn focused(&self) -> Option<&Game> {
        self.games.get(self.focus.as_deref()?)
    }

    pub fn focus_on(&mut self, game_id: String) {
        self.focus = Some(game_id);
    }

    /// Games a match-level notice applies to: the id itself and its synchro halves.
    fn fan_out(&mut self, id: &str) -> Vec<&mut Game> {
        self.games
            .values_mut()
            .filter(|g| g.id == id || g.id.strip_prefix(id).is_some_and(|r| r == ".0" || r == ".1"))
            .collect()
    }

    /// Appends to the kibitz pane of `id` and its synchro boards; returns the lines as notes if none is shown.
    fn add_text(&mut self, id: &str, lines: Vec<String>) -> Vec<String> {
        let games = self.fan_out(id);
        if games.is_empty() {
            return lines.iter().map(|l| format!("{id}: {l}")).collect();
        }
        for g in games {
            g.text.extend(lines.iter().cloned());
        }
        Vec::new()
    }

    /// Applies one `/os` message; returns lines worth showing in the log pane.
    pub fn apply(&mut self, msg: OsMsg, now: Instant) -> Vec<String> {
        let mut notes = Vec::new();
        match msg {
            OsMsg::MatchList(list) => self.matches = list,
            OsMsg::MatchAdd(entry) => {
                let id = match_of(entry.id());
                self.match_observers.remove(id);
                for g in
                    self.games.values_mut().chain(self.restoring_games.values_mut()).filter(|g| match_of(&g.id) == id)
                {
                    g.end_match(Outcome::finished("match replaced".into()));
                }
                self.matches.retain(|e| e.id() != entry.id());
                self.matches.push(entry);
            }
            OsMsg::MatchRemove { id, finished } => {
                self.match_observers.remove(&id);
                self.matches.retain(|e| e.id() != id);
                // The server's watch ends with the match; the finished games stay viewable until unwatched.
                self.watched.retain(|w| *w != id);
                // An end received after stopping still confirms the preceding final updates.
                // Any unconfirmed board leaves the whole match incomplete, including its synchro twin.
                let stale = self
                    .stale
                    .iter()
                    .any(|gid| match_of(gid) == id && self.games.get(gid).is_none_or(Game::takes_moves));
                self.stale.retain(|gid| match_of(gid) != id);
                if stale {
                    self.games.retain(|gid, _| match_of(gid) != id);
                }
                let outcome = finished.as_ref().map_or_else(|| Outcome::finished(String::new()), |f| f.outcome.clone());
                for g in self.fan_out(&id) {
                    g.end_match(outcome.clone());
                }
                if let Some(f) = finished {
                    let boards =
                        self.games.values().filter(|g| match_of(&g.id) == id).map(|g| (g.id.clone(), g.key)).collect();
                    self.finished.push_back(FinishedMatch { report: f, boards });
                    if self.finished.len() > FINISHED_MAX {
                        self.finished.pop_front();
                    }
                }
            }
            OsMsg::Join { id, record } => self.on_join(id, *record, now, &mut notes),
            OsMsg::Update { id, token } => match self.games.get_mut(&id) {
                Some(g) => {
                    if let Err(e) = g.play(token, now) {
                        notes.push(format!("update {id} {}: {e}", token.mv));
                    }
                }
                None => notes.push(format!("update for unknown game {id}")),
            },
            // Players keep getting the notices of a closed own match.
            OsMsg::WatchAdd { id, .. } | OsMsg::WatchRemove { id, .. }
                if self.dismissed.contains(match_of(&id)) && !self.owns_match(match_of(&id)) => {}
            OsMsg::End { id, .. } if self.fan_out(&id).is_empty() => {
                notes.push(format!("end notice for unknown game {id}"));
            }
            OsMsg::End { id, outcome } => {
                for g in self.fan_out(&id) {
                    g.end_board(outcome.clone());
                }
            }
            OsMsg::WatchAdd { name, id } => {
                self.match_observers.entry(match_of(&id).to_string()).or_default().insert(name.clone());
                for g in self.fan_out(&id) {
                    g.observers.insert(name.clone());
                }
            }
            OsMsg::WatchRemove { name, id } => {
                if let Some(observers) = self.match_observers.get_mut(match_of(&id)) {
                    observers.remove(&name);
                }
                for g in self.fan_out(&id) {
                    g.observers.remove(&name);
                }
            }
            OsMsg::Kibitz { id, line } => {
                let mut lines = line.lines;
                if let Some(first) = lines.first_mut() {
                    *first = format!("{}: {first}", line.from);
                }
                notes.extend(self.add_text(&id, lines));
            }
            OsMsg::GameText { id, lines } => notes.extend(self.add_text(&id, lines)),
            OsMsg::Who { mut users, lines } => {
                users.sort_by(|a, b| {
                    (a.status != UserStatus::Open, &a.name).cmp(&(b.status != UserStatus::Open, &b.name))
                });
                let (log, cache) = answered(&mut self.who_asked);
                if log {
                    notes.extend(lines);
                }
                if let Some(t) = cache {
                    self.users_cache.insert(t.key(), users);
                }
            }
            OsMsg::Top { ranks, lines } => {
                let (log, cache) = answered(&mut self.top_asked);
                if log {
                    notes.extend(lines);
                }
                if let Some(t) = cache {
                    self.ranks_cache.insert(t.key(), ranks);
                }
            }
            OsMsg::WatchMissing { ids } => {
                // The match is gone, so no join will confirm a board waiting from before a reconnect.
                for m in ids.iter().map(|id| match_of(id)) {
                    self.match_observers.remove(m);
                    self.watched.retain(|w| w != m);
                    self.restoring_games.retain(|gid, _| match_of(gid) != m);
                    self.stale.retain(|gid| match_of(gid) != m);
                    if !self.games.keys().any(|gid| match_of(gid) == m) {
                        self.release_focus(m);
                    }
                }
                notes.push(format!("not watching {}: no such match", ids.join(" ")));
            }
            OsMsg::Finger { settings, missing } => {
                let entries = missing
                    .into_iter()
                    .map(|login| (login, None))
                    .chain(settings.into_iter().map(|s| (s.login.clone(), Some(s))));
                self.match_settings.extend(entries);
            }
            OsMsg::Look(records) => self.on_look(records, now, &mut notes),
            OsMsg::LookMissing { id } => {
                self.looks.pop_front();
                self.release_focus(&archive_id(&id));
                notes.push(format!("{id} is not in the archive; only rated matches are kept"));
            }
            OsMsg::History { login, games } => {
                self.history.insert(login, games);
            }
            OsMsg::Stored { login, games } => {
                self.stored.insert(login, games);
            }
            OsMsg::Bad { reply, lines, error } => {
                match reply {
                    Some(ReplyKind::Look) => {
                        self.looks.pop_front();
                    }
                    Some(ReplyKind::Who) => {
                        self.who_asked.pop_front();
                    }
                    Some(ReplyKind::Top) => {
                        self.top_asked.pop_front();
                    }
                    None => {}
                }
                notes.push(format!("unparsed {OS} message ({error}):"));
                notes.extend(lines);
            }
            OsMsg::Unknown(lines) => {
                notes.push(format!("unknown {OS} message:"));
                notes.extend(lines);
            }
        }
        notes
    }

    fn on_join(&mut self, id: String, record: GameRecord, now: Instant, notes: &mut Vec<String>) {
        // Players receive their own game even when a stop was sent before its first join.
        if self.dismissed.contains(match_of(&id)) && !self.is_player(&record.black.name, &record.white.name) {
            notes.push(format!("ignored join for stopped {id}"));
            return;
        }
        match Game::from_record(id.clone(), record, now) {
            Ok(mut g) => {
                self.stale.remove(&id);
                if let Some(old) = self.games.remove(&id).filter(|old| old.same_match(&g)) {
                    g.text = old.text;
                    g.key = old.key;
                } else if let Some(old) = self.restoring_games.remove(&id).filter(|old| old.same_match(&g)) {
                    g.key = old.key;
                    g.text = old.text;
                }
                let m = match_of(&id).to_string();
                g.observers = self.match_observers.get(&m).cloned().unwrap_or_default();
                // Boards of the other shape (a bare `.N` against halves `.N.k`) are an earlier match's
                // under a recycled number; same-shape boards wait for their own join.
                let outdated = |gid: &str| match_of(gid) == m && (gid != m) != (id != m);
                self.games.retain(|gid, _| !outdated(gid));
                self.restoring_games.retain(|gid, _| !outdated(gid));
                self.stale.retain(|gid| !outdated(gid));
                add_watch(&mut self.watched, &m);
                // A closed own match keeps arriving; it stays closed until the user reopens it.
                let waiting = !self.dismissed.contains(&m)
                    && self.focused().is_none()
                    && match self.focus.as_deref() {
                        None => !self.background.contains(&m),
                        Some(f) => f == m || f == id,
                    };
                self.games.insert(id.clone(), g);
                if waiting {
                    self.focus_on(id);
                }
            }
            Err(e) => notes.push(format!("join {id}: {e}")),
        }
    }

    fn on_look(&mut self, records: Vec<GameRecord>, now: Instant, notes: &mut Vec<String>) {
        match self.looks.pop_front().map(|(_, key)| key) {
            None => notes.push("look reply that was not asked for".into()),
            Some(key) => {
                let base = archive_id(&key);
                let pair = records.len() > 1;
                let games = records
                    .into_iter()
                    .enumerate()
                    .map(|(i, rec)| {
                        let id = if pair { format!("{base}.{i}") } else { base.clone() };
                        Game::archived(id.clone(), rec, now).map(|mut g| {
                            if let Some(old) = self.games.get(&id).filter(|old| old.matches_record(&g)) {
                                g.key = old.key;
                            }
                            (id, g)
                        })
                    })
                    .collect::<Result<Vec<_>, _>>();
                match games {
                    Ok(games) => self.games.extend(games),
                    Err(e) => notes.push(format!("look {key}: {e}")),
                }
                if self.focus.as_deref() == Some(base.as_str()) {
                    if self.games.keys().any(|g| match_of(g) == base) {
                        self.show(&base);
                    } else {
                        self.focus = None;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
pub(crate) mod tests;
