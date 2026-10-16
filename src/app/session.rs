use super::reads::Reads;
use super::*;

pub(super) struct Conn {
    pub(super) link: Link,
    /// `login@host:port`, or "demo".
    pub(super) label: String,
    pub(super) chat: Chat,
    /// What the connection was opened with; `None` in the demo, which is never retried.
    pub(super) params: Option<LoginParams>,
    /// Retries since the connection was lost; 0 for a connect the user started.
    pub(super) attempt: u32,
    /// When the login completed; `None` until then.
    pub(super) logged_in_at: Option<Instant>,
    pub(super) _pump: Task<()>,
}

/// A lost connection waiting to be opened again.
pub(super) struct Retry {
    pub(super) params: LoginParams,
    pub(super) attempt: u32,
    pub(super) chat: Chat,
    pub(super) _timer: Task<()>,
}

/// 2 s before the first retry, doubling up to a minute.
pub(super) fn retry_delay(attempt: u32) -> Duration {
    Duration::from_secs((2u64 << attempt.saturating_sub(1).min(5)).min(60))
}

/// How long a login must last before its loss restarts the backoff, so a server that drops every
/// login right away is not hit every 2 s.
pub(super) const STABLE_LOGIN: Duration = Duration::from_secs(60);

/// The retry that follows a closed connection, or `None` to go back to the login screen. A first
/// connect that fails is the user's to fix, so only a lost login or a failed retry is retried.
pub(super) fn next_attempt(logged_in_for: Option<Duration>, attempt: u32, retry: bool) -> Option<u32> {
    match (retry, logged_in_for, attempt) {
        (false, ..) | (true, None, 0) => None,
        (true, Some(d), _) if d >= STABLE_LOGIN => Some(1),
        (true, _, n) => Some(n.saturating_add(1)),
    }
}

/// What the window shows: the login form, or a connection in one of its states.
#[derive(Default)]
pub(super) enum Screen {
    #[default]
    Login,
    Connecting(Conn),
    Connected(Conn),
    Reconnecting(Retry),
}

#[derive(Default)]
pub(super) struct Session {
    pub(super) screen: Screen,
    pub(super) lobby: Lobby,
    pub(super) reads: Reads,
    pub(super) watch_cleanup_at: Option<Instant>,
}

impl Session {
    pub(super) fn connected(&self) -> Option<(&Conn, &Lobby)> {
        match &self.screen {
            Screen::Connected(conn) => Some((conn, &self.lobby)),
            _ => None,
        }
    }

    pub(super) fn connected_mut(&mut self) -> Option<(&mut Conn, &mut Lobby)> {
        match &mut self.screen {
            Screen::Connected(conn) => Some((conn, &mut self.lobby)),
            _ => None,
        }
    }

    pub(super) fn lobby_mut(&mut self) -> Option<&mut Lobby> {
        self.connected_mut().map(|(_, lobby)| lobby)
    }
}

impl Viewer {
    pub(super) fn open(&mut self, params: LoginParams, cx: &mut Context<Self>) {
        if let Err(e) = params.validate() {
            self.fail_login(e);
            cx.notify();
            return;
        }
        self.session.lobby = Lobby::default();
        self.session.lobby.me = params.login.clone();
        self.board.views = GameViews::default();
        self.sheet = None;
        self.back.clear();
        self.form.error = None;
        let log = if self.db.is_some() {
            match crate::store::open() {
                Ok(db) => Some((db, format!("{}:{}", params.host, params.port))),
                Err(error) => {
                    note(&mut self.log, format!("could not open the chat log: {error}"));
                    None
                }
            }
        } else {
            None
        };
        let chat = Chat::new(log, params.login.clone());
        self.start(Some(params), chat, 0, cx);
        cx.notify();
    }

    /// Reports background persistence failures without blocking network events or rendering.
    pub(super) fn poll_chat_log(&mut self, now: Instant) -> bool {
        let warning = match &mut self.session.screen {
            Screen::Connecting(conn) | Screen::Connected(conn) => conn.chat.take_log_warning(now),
            Screen::Reconnecting(retry) => retry.chat.take_log_warning(now),
            Screen::Login => None,
        };
        if let Some(warning) = warning {
            note(&mut self.log, warning);
            return true;
        }
        false
    }

    pub(super) fn disconnect(&mut self, cx: &mut Context<Self>) {
        if let Screen::Connecting(conn) | Screen::Connected(conn) = &mut self.session.screen {
            conn.link.close();
        }
        self.session = Session::default();
        self.board.views = GameViews::default();
        self.sheet = None;
        self.back.clear();
        note(&mut self.log, "disconnected");
        cx.notify();
    }

    pub(super) fn send(&mut self, command: impl Into<AppCommand>) -> bool {
        let Some((conn, lobby)) = self.session.connected_mut() else { return false };
        let log = &mut self.log;
        lobby.send_app(command.into(), |line| {
            if !conn.link.send(line.to_string()) {
                return false;
            }
            note(log, if is_secret(line) { "> passw ********".to_string() } else { format!("> {line}") });
            conn.chat.follow_chann(line);
            true
        })
    }

    /// Stops watches no window shows: all of them, or with `closed` only that match, so a window
    /// closing does not cut short the grace of a main-board switch. Each stop is sent through `send`, so
    /// the log pane shows it.
    pub(super) fn stop_unused_watches(&mut self, closed: Option<&str>, cx: &mut Context<Self>) {
        if closed.is_none() {
            self.session.watch_cleanup_at = None;
        }
        let windows: BTreeSet<String> = self.match_windows.keys().cloned().collect();
        let tiles = self.tiles();
        let keep = |focus: &Option<String>, id: &str| {
            windows.contains(id)
                || tiles.iter().any(|t| t == id)
                || focus.as_deref().is_some_and(|f| match_of(f) == id)
                || closed.is_some_and(|c| c != id)
        };
        let focus = self.session.lobby.focus.clone();
        if matches!(self.session.screen, Screen::Connected(_)) {
            let commands = self.session.lobby.retain_watches(|id| keep(&focus, id));
            for cmd in commands {
                self.send(cmd);
            }
        } else {
            self.session.lobby.discard_watches(|id| keep(&focus, id));
        }
        cx.notify();
    }

    pub(super) fn start(&mut self, params: Option<LoginParams>, chat: Chat, attempt: u32, cx: &mut Context<Self>) {
        self.session.reads = Reads::default();
        self.session.watch_cleanup_at = None;
        let (tx, rx) = async_channel::bounded(64);
        let (link, label) = match &params {
            Some(p) => (net::connect(p.clone(), tx), format!("{}@{}:{}", p.login, p.host, p.port)),
            None => (net::demo(tx), "demo".into()),
        };
        let pump = cx.spawn(async move |this, cx| {
            while let Ok(ev) = rx.recv().await {
                if this.update(cx, |v, cx| v.on_net(ev, cx)).is_err() {
                    break;
                }
            }
        });
        self.session.screen =
            Screen::Connecting(Conn { link, label, chat, params, attempt, logged_in_at: None, _pump: pump });
    }

    fn reconnect_later(&mut self, params: LoginParams, attempt: u32, chat: Chat, reason: &str, cx: &mut Context<Self>) {
        let delay = retry_delay(attempt);
        note(
            &mut self.log,
            format!("connection lost: {reason}; reconnecting in {} s (attempt {attempt})", delay.as_secs()),
        );
        let timer = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(delay).await;
            let _ = this.update(cx, |v, cx| {
                if let Screen::Reconnecting(r) = std::mem::replace(&mut v.session.screen, Screen::Login) {
                    v.start(Some(r.params), r.chat, r.attempt, cx);
                    cx.notify();
                }
            });
        });
        self.session.screen = Screen::Reconnecting(Retry { params, attempt, chat, _timer: timer });
    }

    pub(super) fn on_net(&mut self, ev: NetEvent, cx: &mut Context<Self>) {
        match ev {
            NetEvent::Status(s) => note(&mut self.log, s),
            NetEvent::LoggedIn => {
                let mut rejoin = Vec::new();
                if let Screen::Connecting(mut conn) = std::mem::replace(&mut self.session.screen, Screen::Login) {
                    conn.logged_in_at = Some(Instant::now());
                    // Only a login that worked is offered again.
                    if conn.attempt == 0
                        && let Some(p) = &conn.params
                    {
                        self.form.history.record(Entry {
                            host: p.host.clone(),
                            port: p.port.to_string(),
                            login: p.login.clone(),
                        });
                        if let Some(db) = &self.db
                            && let Err(e) = self.form.history.save(db)
                        {
                            note(&mut self.log, format!("could not save the login history: {e}"));
                        }
                    }
                    if conn.attempt > 0 {
                        // Channels joined on the lost connection; .chat is joined at every login.
                        rejoin = conn
                            .chat
                            .conversations
                            .iter()
                            .filter_map(|c| match &c.key {
                                ChatKey::Channel(ch) if ch != DEFAULT_CHANNEL => Some(format!("chann + {ch}")),
                                _ => None,
                            })
                            .collect();
                    }
                    self.session.screen = Screen::Connected(conn);
                }
                note(&mut self.log, "logged in; waiting for /os");
                if self.session.connected().is_some_and(|(conn, _)| conn.params.is_none()) {
                    self.send(AppCommand::Who(RatingType::Eight));
                }
                for cmd in rejoin {
                    self.send(cmd);
                }
            }
            NetEvent::Message(msg) => self.on_message(msg, cx),
            NetEvent::Closed { reason, retry } => {
                let conn = match std::mem::replace(&mut self.session.screen, Screen::Login) {
                    Screen::Connected(conn) | Screen::Connecting(conn) => conn,
                    screen => {
                        self.session.screen = screen;
                        return;
                    }
                };
                self.session.lobby.connection_lost();
                match (conn.params, next_attempt(conn.logged_in_at.map(|t| t.elapsed()), conn.attempt, retry)) {
                    (Some(params), Some(attempt)) => self.reconnect_later(params, attempt, conn.chat, &reason, cx),
                    _ => self.fail_login(format!("disconnected: {reason}")),
                }
                // The link is gone; trim subscriptions before the next connection.
                if self.session.watch_cleanup_at.is_some() {
                    self.stop_unused_watches(None, cx);
                }
            }
        }
        cx.notify();
    }

    pub(super) fn on_message(&mut self, msg: Vec<String>, cx: &mut Context<Self>) {
        if let Some((_, lobby)) = self.session.connected_mut()
            && let Some(ran) = proto::ack(&msg)
        {
            lobby.acked(ran);
        }
        match proto::parse(msg) {
            ServerMsg::Presence { os: Some(true) } => {
                let Some((_, lobby)) = self.session.connected_mut() else { return };
                if lobby.os == OsState::Present {
                    return;
                }
                lobby.os = OsState::Present;
                note(&mut self.log, "/os is present");
                let mut cmds: Vec<AppCommand> = proto::os_hello().into_iter().map(Into::into).collect();
                cmds.extend(lobby.reconnect_commands());
                for cmd in cmds {
                    self.send(cmd);
                }
                self.refresh_tourneys();
                // suspend() dropped the lists the open rail and Games sheet show.
                self.refresh_rail(false, cx);
                if let Some(Sheet::Games(login)) = &self.sheet {
                    self.open_games(login.clone(), cx);
                }
            }
            ServerMsg::Presence { os: Some(false) } => {
                self.session.reads.sent.clear();
                note(&mut self.log, "/os departed");
                if let Some((_, lobby)) = self.session.connected_mut() {
                    lobby.suspend();
                }
            }
            ServerMsg::Presence { os: None } => {}
            ServerMsg::Os(m) => {
                let joined = match &m {
                    OsMsg::Join { id, .. } => Some(id.clone()),
                    _ => None,
                };
                if match &m {
                    OsMsg::MatchAdd(MatchEntry::Summary(s)) => s.mode == "T",
                    OsMsg::MatchRemove { finished: Some(f), .. } => f.summary.mode == "T",
                    _ => false,
                } {
                    self.tourney_changed();
                }
                if let Some((conn, lobby)) = self.session.connected_mut() {
                    if let OsMsg::Kibitz { id, line } = &m {
                        conn.chat.add(ChatKey::Game(id.clone()), line.clone(), SystemTime::now());
                        self.kibitz_scroll.scroll_to_bottom();
                    }
                    note_block(&mut self.log, lobby.apply(m, Instant::now()));
                    if let Some(game) = joined.as_ref().and_then(|id| lobby.games.get(id)) {
                        self.board.views.clamp(game);
                        if let Some((_, child)) = self.match_windows.get(match_of(&game.id)) {
                            let _ = child.update(cx, |child, _| child.clamp_view(game, lobby));
                        }
                    }
                }
                // Inherited tracking can subscribe without a UI action. Use the same fixed grace
                // as a main-board switch; another join must not postpone cleanup indefinitely.
                if joined.is_some() {
                    self.session.watch_cleanup_at.get_or_insert_with(Instant::now);
                }
            }
            ServerMsg::Help(h) => {
                note_block(&mut self.log, std::iter::once(h.title.clone()).chain(h.lines.iter().cloned()).collect());
                // The server also sends the FAQ, help and news unasked at login; those stay in the log.
                if matches!(self.sheet, Some(Sheet::Help(_))) {
                    self.sheet = Some(Sheet::Help(Some(h)));
                }
            }
            ServerMsg::Chat { key, line } => {
                note_block(&mut self.log, vec![format!("{} {}: {}", key.label(), line.from, line.lines.join(" / "))]);
                // An engine's report goes to its game rather than the chat, where every move would mark it unread.
                // A report on a game not shown is dropped.
                let report = match key {
                    ChatKey::Channel(_) => search::parse(&line),
                    _ => None,
                };
                if let Some(log) = report {
                    if let Some((_, lobby)) = self.session.connected_mut()
                        && let Some(game) = lobby.games.get_mut(&log.game)
                    {
                        game.add_search(log);
                    }
                } else if let Screen::Connected(conn) = &mut self.session.screen {
                    self.attention |= conn.chat.add(key, line, SystemTime::now());
                    if self.rail == Rail::Chat {
                        self.chat_scroll.scroll_to_end();
                    }
                }
            }
            ServerMsg::Finger { profiles, missing, lines } => {
                note_block(&mut self.log, lines);
                if let Some((_, lobby)) = self.session.connected_mut() {
                    lobby.profiles.extend(missing.into_iter().map(|login| (login, None)));
                    lobby.profiles.extend(profiles.into_iter().map(|p| (p.login.clone(), Some(p))));
                }
            }
            ServerMsg::OsAbsent(lines) => {
                note_block(&mut self.log, lines);
                // Nothing answers the requests made while /os is away; its arrival asks again.
                if let Some((_, lobby)) = self.session.connected_mut() {
                    lobby.suspend();
                }
            }
            ServerMsg::Td(m) => {
                match &m {
                    TdMsg::Error { code, command, text } => {
                        note(&mut self.log, format!("/td: ERR {code}: {command}: {text}"))
                    }
                    TdMsg::Unknown(lines) => note_block(&mut self.log, lines.clone()),
                    _ => {}
                }
                let tourneys = &mut self.session.lobby.tourneys;
                // The list may announce the start before finger arrives. Missing results still
                // need a request, even when another reply already marked the tournament started.
                let missing_results = match (&m, &self.sheet) {
                    (TdMsg::Info(info), Some(Sheet::Tourney(id)))
                        if info.id == *id
                            && info.started()
                            && !(tourneys.schedules.contains_key(id) && tourneys.standings.contains_key(id)) =>
                    {
                        Some(id.clone())
                    }
                    _ => None,
                };
                tourneys.apply(m, Instant::now());
                if let Some(id) = missing_results {
                    self.send_read(tourney_results_read(&id));
                }
            }
            ServerMsg::TdNews(lines) => {
                note_block(&mut self.log, lines);
                self.tourney_changed();
            }
            ServerMsg::Other(lines) => note_block(&mut self.log, lines),
        }
        // A schedule or a `+ match` can add a tile.
        self.watch_tiles();
    }
}
