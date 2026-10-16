use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::time::{Duration, Instant, SystemTime};

use gpui_kit::base::{ElementExt, SelectableText, TestSupportExt, TextView, TextViewState, TextViewStyle};
use gpui_kit::component::button::*;
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::menu::{DropdownMenu, PopupMenuItem};
use gpui_kit::component::resizable::{ResizableState, h_resizable, resizable_panel, v_resizable};
use gpui_kit::component::scroll::ScrollableElement;
use gpui_kit::component::tab::{Tab, TabBar};
use gpui_kit::component::{Icon, IconName, Sizable, StyledExt};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use rusqlite::Connection;

use crate::chat::{Chat, ChatKey, ChatLine, Conversation, DEFAULT_CHANNEL, Said, Unread};
use crate::game::{At, Game, GameSource, GameViews, Nav, Phase};
use crate::history::{Entry, History};
use crate::lobby::{AppCommand, FinishedMatch, Lobby, OsState, ViewMode, games_of, match_of};
use crate::net::{self, Link, LoginParams, NetEvent, is_secret};
use crate::othello::{Cell, Color, Square};
use crate::proto::{
    self, Archived, HelpText, MatchEntry, MatchSettings, OsMsg, Profile, Rank, RatingType, Seen, ServerMsg, User,
    UserStatus,
};
use crate::search;
use crate::td::{self, TdMsg};
use crate::ui;

const LOG_CAP: usize = 500;
/// Every viewer hears the same /td news at once; a random extra wait spreads their refreshes out.
const TD_REFRESH_JITTER: Duration = Duration::from_secs(2);

mod login;
use login::LoginForm;

mod lists;
use lists::*;

mod session;
use session::*;

mod board;
use board::*;

mod reads;

mod credentials;
mod match_window;

struct LogBlock {
    /// Stable across insertions at the front, so an arriving message keeps the selected text.
    id: usize,
    lines: Vec<String>,
}

#[derive(Clone, Copy, PartialEq)]
enum Rail {
    Matches,
    Users,
    Rankings,
    Tourneys,
    Chat,
}

/// The rail tabs in order.
const RAILS: [(Rail, &str); 5] = [
    (Rail::Matches, "Matches"),
    (Rail::Users, "Users"),
    (Rail::Rankings, "Top"),
    (Rail::Tourneys, "Tourney"),
    (Rail::Chat, "Chat"),
];

/// What the window does on start.
pub enum Launch {
    Login,
    Demo,
    Connect(LoginParams),
}

/// A page shown in place of the board until it is closed.
enum Sheet {
    /// The `history` and `stored` lists of one login.
    Games(String),
    /// A help file; `None` until the reply arrives.
    Help(Option<HelpText>),
    /// One /td tournament by id.
    Tourney(String),
}

impl Sheet {
    /// The same page, whatever has arrived for it since.
    fn same_page(&self, other: &Sheet) -> bool {
        match (self, other) {
            (Sheet::Games(a), Sheet::Games(b)) | (Sheet::Tourney(a), Sheet::Tourney(b)) => a == b,
            (Sheet::Help(_), Sheet::Help(_)) => true,
            _ => false,
        }
    }
}

/// How many pages Back remembers.
const BACK_CAP: usize = 20;

/// The help files the Help page offers.
const HELP_FILES: [(&str, &str); 3] =
    [("GGS help", "help"), ("News", "help news"), ("Othello service", "tell /os help")];

/// Sent lines for Up and Down, with the unfinished input restored at the end.
#[derive(Default)]
struct Recall {
    lines: Vec<String>,
    /// The line being shown, or `None` when editing a new one.
    at: Option<usize>,
    draft: String,
}

impl Recall {
    fn push(&mut self, line: String) {
        if self.lines.last() != Some(&line) {
            self.lines.push(line);
        }
        self.at = None;
        self.draft.clear();
    }

    /// The previous line, stopping at the oldest.
    fn up(&mut self, draft: &str) -> Option<&str> {
        let i = match self.at {
            None => {
                let i = self.lines.len().checked_sub(1)?;
                self.draft = draft.to_string();
                i
            }
            Some(i) => i.saturating_sub(1),
        };
        self.at = Some(i);
        Some(&self.lines[i])
    }

    /// The next line, or the unfinished input after the newest.
    fn down(&mut self) -> Option<&str> {
        let i = self.at? + 1;
        self.at = (i < self.lines.len()).then_some(i);
        Some(self.lines.get(i).map_or(self.draft.as_str(), String::as_str))
    }
}

pub struct Viewer {
    form: LoginForm,
    /// `ggs.db` for the history and the theme; `None` when it could not be opened, and nothing is saved.
    db: Option<Connection>,
    session: Session,
    /// Messages and notes, newest first; a server message keeps its lines together.
    log: VecDeque<LogBlock>,
    show_log: bool,
    /// The line for the open chat conversation.
    chat_input: Entity<InputState>,
    chat_recall: Recall,
    /// A channel to join from the Chat tab.
    channel_input: Entity<InputState>,
    chat_scroll: ListState,
    chat_text: Entity<TextViewState>,
    /// The conversation `chat_text` holds and how many of its lines.
    chat_shown: (Option<ChatKey>, usize),
    kibitz_input: Entity<InputState>,
    kibitz_recall: Recall,
    /// Kibitz goes to the observers only (`tell o`), not to the players too.
    kibitz_observers: bool,
    kibitz_scroll: ScrollHandle,
    center_scroll: ScrollHandle,
    rail: Rail,
    /// The rating type the Users and Rankings tabs ask for.
    rating_type: RatingType,
    sheet: Option<Sheet>,
    /// The pages left for another page or a board, newest last, offered again by the Back button.
    back: Vec<Sheet>,
    /// The tournament whose standings are shown as they stood after a past round, and that round.
    tourney_round: Option<(String, u32)>,
    /// The round picker beside the standings, which steps through the rounds with Up and Down.
    round_focus: FocusHandle,
    round_menu_open: bool,
    focus_layout: Entity<ResizableState>,
    plain_layout: Entity<ResizableState>,
    log_layout: Entity<ResizableState>,
    moves_layout: Entity<ResizableState>,
    /// The viewport size and the details pane the side and log groups were last laid out for.
    laid_out_for: Option<(Size<Pixels>, bool)>,
    /// Actual center viewport after panel constraints and the view toolbar are laid out.
    center_size: Option<Size<Pixels>>,
    board: BoardPane,
    focus: FocusHandle,
    /// A tell or a mention arrived; the window asks for attention at the next draw if it is in the background.
    attention: bool,
    theme: ui::ThemeChoice,
    /// Saved when the window closes.
    layout: ui::Layout,
    match_windows: BTreeMap<String, (AnyWindowHandle, WeakEntity<match_window::MatchWindow>)>,
    _ticker: Task<()>,
    _subscriptions: Vec<Subscription>,
}

fn note(log: &mut VecDeque<LogBlock>, line: impl Into<String>) {
    note_block(log, vec![line.into()]);
}

fn note_block(log: &mut VecDeque<LogBlock>, lines: Vec<String>) {
    if !lines.is_empty() {
        let id = log.front().map_or(0, |b| b.id + 1);
        log.push_front(LogBlock { id, lines });
        log.truncate(LOG_CAP);
    }
}

fn text(state: &Entity<InputState>, cx: &App) -> String {
    state.read(cx).value().trim().to_string()
}

/// Runs `act` on the trimmed line when Enter is pressed in `input`, and clears the field when it returns true.
fn on_enter(
    input: &Entity<InputState>,
    window: &mut Window,
    cx: &mut Context<Viewer>,
    act: impl Fn(&mut Viewer, String, &mut Context<Viewer>) -> bool + 'static,
) -> Subscription {
    cx.subscribe_in(input, window, move |this, state, ev: &InputEvent, window, cx| {
        if let InputEvent::PressEnter { .. } = ev {
            let line = text(state, cx);
            if !line.is_empty() && act(this, line, cx) {
                state.update(cx, |s, cx| s.set_value("", window, cx));
            }
        }
    })
}

impl Viewer {
    pub fn new(
        launch: Launch,
        theme: ui::ThemeChoice,
        db: rusqlite::Result<Connection>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let field = |value: &str, masked: bool, window: &mut Window, cx: &mut Context<Self>| {
            cx.new(|cx| InputState::new(window, cx).default_value(value.to_string()).masked(masked))
        };
        let ticker = cx.spawn(async move |this, cx| {
            loop {
                // 0.98 s, so the displayed second never skips.
                cx.background_executor().timer(Duration::from_millis(980)).await;
                let tick = |v: &mut Viewer, cx: &mut Context<Viewer>| {
                    v.release_tiles();
                    if v.session.watch_cleanup_at.is_some_and(|at| at.elapsed() >= Duration::from_secs(5)) {
                        v.stop_unused_watches(None, cx);
                    }
                    if v.session.reads.refresh_due(Instant::now()) {
                        v.refresh_tourneys();
                    }
                    let chat_warning = v.poll_chat_log(Instant::now());
                    if chat_warning || matches!(v.session.screen, Screen::Connected(..)) && v.has_countdown() {
                        cx.notify()
                    }
                };
                if this.update(cx, tick).is_err() {
                    break;
                }
            }
        });
        let weak = cx.weak_entity();
        let on_match_window_closed = cx.on_window_closed(move |cx, closed| {
            let _ = weak.update(cx, |this, cx| {
                let ids: Vec<String> = this
                    .match_windows
                    .iter()
                    .filter(|(_, (handle, _))| handle.window_id() == closed)
                    .map(|(id, _)| id.clone())
                    .collect();
                this.match_windows.retain(|_, (handle, _)| handle.window_id() != closed);
                for id in ids {
                    this.stop_unused_watches(Some(&id), cx);
                }
            });
        });
        let (db, db_error) = match db {
            Ok(db) => (Some(db), None),
            Err(e) => (None, Some(format!("could not open ggs.db, so logins, theme and chat are not saved: {e}"))),
        };
        let history = db.as_ref().and_then(|db| History::load(db).ok()).unwrap_or_default();
        let layout = db.as_ref().map_or_else(ui::Layout::default, ui::Layout::load);
        let weak = cx.weak_entity();
        window.on_window_should_close(cx, move |window, cx| {
            let bounds = window.window_bounds();
            let _ = weak.update(cx, |this, _| this.save_layout(bounds));
            true
        });
        let (host, port, login, password) = match (&launch, history.latest().cloned()) {
            (Launch::Connect(p), _) => (p.host.clone(), p.port.to_string(), p.login.clone(), p.password.clone()),
            (_, Some(l)) => (l.host, l.port, l.login, String::new()),
            (_, None) => ("www.skatgame.net".into(), "5000".into(), String::new(), String::new()),
        };
        let chat_input = cx.new(|cx| InputState::new(window, cx).placeholder("Message"));
        let on_chat = on_enter(&chat_input, window, cx, |this, text, _| this.send_chat(text));
        let channel_input = cx.new(|cx| InputState::new(window, cx).placeholder("Join a channel, e.g. .tourney"));
        let on_channel = on_enter(&channel_input, window, cx, |this, value, cx| {
            let Some(name) = value
                .split_whitespace()
                .next()
                .map(|n| if n.starts_with('.') { n.to_string() } else { format!(".{n}") })
            else {
                return false;
            };
            if !matches!(this.session.screen, Screen::Connected(..)) {
                return false;
            }
            this.send(format!("chann + {name}"));
            this.open_chat(ChatKey::Channel(name), cx);
            true
        });
        let kibitz_input = cx.new(|cx| InputState::new(window, cx).placeholder("Kibitz"));
        let on_kibitz = on_enter(&kibitz_input, window, cx, |this, text, cx| this.send_kibitz(text, cx));
        // The input binds Up and Down to cursor moves, so they are taken before key bindings run.
        let weak = cx.weak_entity();
        let on_arrow = cx.intercept_keystrokes(move |ev, window, cx| {
            let up = match ev.keystroke.key.as_str() {
                "up" => true,
                "down" => false,
                _ => return,
            };
            let _ = weak.update(cx, |this, cx| this.recall_line(up, window, cx));
        });
        let host = field(&host, false, window, cx);
        let port = field(&port, false, window, cx);
        let login = field(&login, false, window, cx);
        let password = field(&password, true, window, cx);
        let identity_changed = |this: &mut Viewer,
                                _: &Entity<InputState>,
                                ev: &InputEvent,
                                window: &mut Window,
                                cx: &mut Context<Viewer>| {
            if let InputEvent::Change = ev {
                this.restore_password(window, cx);
                cx.notify();
            }
        };
        let on_host = cx.subscribe_in(&host, window, identity_changed);
        let on_port = cx.subscribe_in(&port, window, identity_changed);
        let on_login = cx.subscribe_in(&login, window, identity_changed);
        let on_password = cx.subscribe(&password, |this, _, ev: &InputEvent, cx| match ev {
            InputEvent::Change => {
                this.form.credential_generation += 1;
                this.form.restored_password = false;
                cx.notify();
            }
            // Not `on_enter`: an empty password is a valid guest login.
            InputEvent::PressEnter { .. } if matches!(this.session.screen, Screen::Login) => this.connect(cx),
            _ => {}
        });
        let on_appearance = cx.observe_window_appearance(window, |this, window, cx| {
            if this.theme == ui::ThemeChoice::System {
                ui::install(this.theme.is_light(window.appearance()), cx);
            }
        });
        let chat_text = cx.new(|cx| TextViewState::markdown("", cx));
        let chat_scroll = chat_text.read(cx).list_state().clone();
        chat_scroll.set_follow_mode(FollowMode::Tail);
        let mut viewer = Viewer {
            form: LoginForm {
                host,
                port,
                login,
                password,
                remember_password: false,
                restored_password: false,
                credential_generation: 0,
                credentials: credentials::Credentials::default(),
                credential_task: None,
                history,
                error: None,
            },
            db,
            session: Session::default(),
            log: VecDeque::new(),
            show_log: false,
            chat_input,
            chat_recall: Recall::default(),
            channel_input,
            chat_scroll,
            chat_text,
            chat_shown: (None, 0),
            kibitz_input,
            kibitz_recall: Recall::default(),
            kibitz_observers: false,
            kibitz_scroll: ScrollHandle::new(),
            center_scroll: ScrollHandle::new(),
            rail: Rail::Matches,
            rating_type: RatingType::Eight,
            sheet: None,
            back: Vec::new(),
            tourney_round: None,
            round_focus: cx.focus_handle(),
            round_menu_open: false,
            focus_layout: cx.new(|_| ResizableState::default()),
            plain_layout: cx.new(|_| ResizableState::default()),
            log_layout: cx.new(|_| ResizableState::default()),
            moves_layout: cx.new(|_| ResizableState::default()),
            laid_out_for: None,
            center_size: None,
            board: BoardPane::default(),
            focus: cx.focus_handle(),
            attention: false,
            theme,
            layout,
            match_windows: BTreeMap::new(),
            _ticker: ticker,
            _subscriptions: vec![
                on_host,
                on_port,
                on_login,
                on_password,
                on_chat,
                on_channel,
                on_kibitz,
                on_arrow,
                on_appearance,
                on_match_window_closed,
            ],
        };
        if let Some(e) = db_error {
            viewer.fail_login(e);
        }
        match launch {
            Launch::Demo => viewer.start(None, Chat::new(None, "demo".into()), 0, cx),
            Launch::Connect(p) => viewer.open(p, cx),
            Launch::Login => viewer.restore_password(window, cx),
        }
        viewer
    }

    /// Shows `text` in the open conversation only after the connection queue accepts it.
    fn send_chat(&mut self, text: String) -> bool {
        let Screen::Connected(conn) = &mut self.session.screen else { return false };
        let (cmd, echo) = match &conn.chat.selected {
            ChatKey::Channel(c) => (format!("tell {c} {text}"), true),
            // A tell to oneself comes back from the server, so it is shown from there.
            ChatKey::User(u) => (format!("tell {u} {text}"), u != conn.chat.me()),
            ChatKey::Game(_) => return false,
        };
        if !self.send(cmd) {
            return false;
        }
        if echo && let Screen::Connected(conn) = &mut self.session.screen {
            let key = conn.chat.selected.clone();
            conn.chat.add(
                key,
                ChatLine { from: conn.chat.me().to_string(), lines: vec![text.clone()] },
                SystemTime::now(),
            );
        }
        self.chat_recall.push(text);
        self.chat_scroll.scroll_to_end();
        true
    }

    /// Kibitzes on the focused live game.
    fn send_kibitz(&mut self, text: String, cx: &mut Context<Self>) -> bool {
        let Some((conn, lobby)) = self.session.connected_mut() else { return false };
        let Some(id) = lobby.focused().filter(|g| lobby.can_kibitz(g)).map(|g| g.id.clone()) else {
            return false;
        };
        let cmd = AppCommand::Kibitz {
            id: id.clone(),
            me: conn.chat.me().into(),
            text: text.clone(),
            observers_only: self.kibitz_observers,
        };
        if !self.send(cmd) {
            return false;
        }
        if let Screen::Connected(conn) = &mut self.session.screen {
            conn.chat.add(
                ChatKey::Game(id),
                ChatLine { from: conn.chat.me().to_string(), lines: vec![text.clone()] },
                SystemTime::now(),
            );
        }
        self.kibitz_recall.push(text);
        cx.notify();
        self.kibitz_scroll.scroll_to_bottom();
        true
    }

    fn open_chat(&mut self, key: ChatKey, cx: &mut Context<Self>) {
        if let Screen::Connected(conn) = &mut self.session.screen {
            conn.chat.open(key);
        }
        self.rail = Rail::Chat;
        self.chat_scroll.scroll_to_end();
        cx.notify();
    }

    fn recall_line(&mut self, up: bool, window: &mut Window, cx: &mut Context<Self>) {
        let (input, recall) = if self.chat_input.read(cx).focus_handle(cx).is_focused(window) {
            (&self.chat_input, &mut self.chat_recall)
        } else if self.kibitz_input.read(cx).focus_handle(cx).is_focused(window) {
            (&self.kibitz_input, &mut self.kibitz_recall)
        } else {
            return;
        };
        let draft = input.read(cx).value();
        let line = if up { recall.up(&draft) } else { recall.down() };
        if let Some(line) = line.map(str::to_string) {
            input.update(cx, |s, cx| s.set_value(line, window, cx));
        }
        cx.stop_propagation();
    }

    fn open_help(&mut self, cmd: &str, cx: &mut Context<Self>) {
        self.show_sheet(Sheet::Help(None));
        self.send(cmd.to_string());
        cx.notify();
    }

    fn with_lobby(&mut self, cx: &mut Context<Self>, f: impl FnOnce(&mut Lobby) -> Vec<AppCommand>) {
        if let Some((_, lobby)) = self.session.connected_mut() {
            let before = lobby.focus.as_deref().map(|f| match_of(f).to_string());
            let cmds = f(lobby);
            // A main-board switch starts the countdown to stop the watch it left.
            if before.is_some() && before.as_deref() != lobby.focus.as_deref().map(match_of) {
                self.session.watch_cleanup_at.get_or_insert_with(Instant::now);
            }
            for cmd in cmds {
                self.send(cmd);
            }
            cx.notify();
        }
    }

    /// Shows `login`'s page with what is loaded and asks for it afresh.
    fn open_games(&mut self, login: String, cx: &mut Context<Self>) {
        if self.allow_read(format!("games {login}")) {
            for cmd in games_of(&login) {
                self.send(cmd);
            }
        }
        self.show_sheet(Sheet::Games(login));
        cx.notify();
    }

    /// Shows tournament `id` and asks for what it shows.
    fn open_tourney(&mut self, id: String, cx: &mut Context<Self>) {
        let reads = if self.session.lobby.tourneys.settled(&id) { Vec::new() } else { self.tourney_sheet_reads(&id) };
        for read in reads {
            if !self.send_read(read) {
                break;
            }
        }
        self.show_sheet(Sheet::Tourney(id));
        self.watch_tiles();
        cx.notify();
    }

    /// Sends a read's commands unless its key is cooling down; false if it is.
    fn send_read(&mut self, (key, cmds): (String, Vec<String>)) -> bool {
        if !self.allow_read(key) {
            return false;
        }
        for cmd in cmds {
            self.send(cmd);
        }
        true
    }

    /// Tournament `id`'s sheet reads: its info, then its schedule and standings once it has started.
    fn tourney_sheet_reads(&self, id: &str) -> Vec<(String, Vec<String>)> {
        let mut reads = vec![(format!("td {id}"), vec![td::tell(&format!("finger {id}"))])];
        if self.session.lobby.tourneys.started(id) {
            reads.push(tourney_results_read(id));
        }
        reads
    }

    /// The matches the open tournament shows as tiles.
    fn tiles(&self) -> Vec<String> {
        match &self.sheet {
            Some(Sheet::Tourney(id)) => tile_matches(&self.session.lobby, id),
            _ => Vec::new(),
        }
    }

    /// Watches the open tournament's running tiles without touching the board.
    fn watch_tiles(&mut self) {
        for m in self.tiles() {
            // A finished game's tile keeps its boards; its match is over and has nothing to watch.
            if !self.session.lobby.matches.iter().any(|e| e.id() == m) {
                continue;
            }
            let cmds = self.session.lobby.request_watch(&m);
            if !cmds.is_empty() {
                self.session.lobby.background.insert(m);
            }
            for cmd in cmds {
                self.send(cmd);
            }
        }
    }

    /// Tiles that are gone, with their sheet or their game, start the grace that stops their watches.
    fn release_tiles(&mut self) {
        let tiles = self.tiles();
        let background = &mut self.session.lobby.background;
        let before = background.len();
        background.retain(|m| tiles.contains(m));
        if background.len() < before {
            self.session.watch_cleanup_at.get_or_insert_with(Instant::now);
        }
    }

    /// Refreshes each visible resource once for the pending events, respecting its own cooldown.
    fn refresh_tourneys(&mut self) {
        if !matches!(self.session.screen, Screen::Connected(_)) {
            self.session.reads.td_refresh = None;
            return;
        }
        for command in self.session.reads.refresh(self.tourney_reads(), Instant::now()) {
            self.send(command);
        }
    }

    /// The tournament reads on screen, each keyed for `allow_read` with the commands it sends.
    fn tourney_reads(&self) -> Vec<(String, Vec<String>)> {
        let mut requests = Vec::new();
        if self.rail == Rail::Tourneys {
            requests.push(("tourneys".to_string(), vec![td::tell("tourneys")]));
        }
        if let Some(Sheet::Tourney(id)) = &self.sheet {
            requests.extend(self.tourney_sheet_reads(id));
        }
        requests
    }

    fn tourney_changed(&mut self) {
        if self.rail == Rail::Tourneys || matches!(self.sheet, Some(Sheet::Tourney(_))) {
            self.session.reads.tourney_changed(Instant::now(), jitter());
        }
    }

    /// Shows a page, keeping the one it replaces for the Back button; reopening the same page refreshes it in place.
    fn show_sheet(&mut self, sheet: Sheet) {
        if let Some(previous) = self.sheet.take().filter(|s| !s.same_page(&sheet)) {
            self.remember(previous);
        }
        self.sheet = Some(sheet);
    }

    /// Leaves the sheet for a board, keeping it for the Back button.
    fn leave_sheet(&mut self) {
        if let Some(sheet) = self.sheet.take() {
            self.remember(sheet);
        }
    }

    fn remember(&mut self, sheet: Sheet) {
        if self.back.len() == BACK_CAP {
            self.back.remove(0);
        }
        self.back.push(sheet);
    }

    fn back_button(&self, cx: &mut Context<Self>) -> Option<Stateful<Div>> {
        self.back.last().map(|back| {
            ui::button("back", back_label(back), false, true)
                .debug_selector(|| "back".into())
                .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.go_back(cx)))
        })
    }

    fn go_back(&mut self, cx: &mut Context<Self>) {
        let Some(previous) = self.back.pop() else { return };
        self.sheet = None;
        match previous {
            Sheet::Tourney(id) => self.open_tourney(id, cx),
            Sheet::Games(login) => self.open_games(login, cx),
            sheet => {
                self.sheet = Some(sheet);
                cx.notify();
            }
        }
    }

    fn look(&mut self, key: String, cx: &mut Context<Self>) {
        self.leave_sheet();
        self.with_lobby(cx, |l| l.look(&key));
    }

    fn watch(&mut self, id: &str, cx: &mut Context<Self>) {
        self.leave_sheet();
        self.with_lobby(cx, |l| l.watch(id));
    }

    /// Whether anything on screen counts down: a live game's clock or a tournament's start.
    fn has_countdown(&self) -> bool {
        self.rail == Rail::Tourneys
            || matches!(self.sheet, Some(Sheet::Tourney(_)))
            || self.session.lobby.games.values().any(|g| g.phase == Phase::Live)
    }

    fn copy_ggf(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some((_, lobby)) = self.session.connected() else { return };
        let Some(g) = lobby.games.get(id) else { return };
        cx.write_to_clipboard(ClipboardItem::new_string(g.to_ggf()));
        note(&mut self.log, format!("copied the GGF of {id}"));
        cx.notify();
    }

    fn flip(&mut self, cx: &mut Context<Self>) {
        let Some(lobby) = self.session.lobby_mut() else { return };
        if lobby.view == ViewMode::Synchro {
            return;
        }
        if let Some(game) = lobby.focused() {
            let view = self.board.views.get_mut(game);
            view.flipped = !view.flipped;
        }
        cx.notify();
    }

    fn navigate(&mut self, nav: Nav, cx: &mut Context<Self>) {
        let Some(l) = self.session.lobby_mut() else { return };
        if l.view == ViewMode::Synchro {
            return;
        }
        if let Some(game) = l.focused() {
            self.board.views.navigate(game, nav);
        }
        if let Some(r) = l.focused().map(|g| record_row(g, self.board.views.get(g).cursor)) {
            self.board.moves_scroll.scroll_to_item(r);
        }
        cx.notify();
    }

    fn allow_read(&mut self, key: String) -> bool {
        matches!(self.session.screen, Screen::Connected(..)) && self.session.reads.allow(key, Instant::now())
    }

    /// Shows the rail's loaded list and asks for it afresh; false if there is nothing to show.
    /// Matches stays current through live notifications, so only `refresh` asks for it.
    fn refresh_rail(&mut self, refresh: bool, cx: &mut Context<Self>) -> bool {
        cx.notify();
        let (rail, t) = (self.rail, self.rating_type);
        if !refresh && rail == Rail::Matches {
            return true;
        }
        let loaded = self.session.lobby_mut().is_some_and(|lobby| match rail {
            Rail::Users => lobby.show_users(t, Instant::now()),
            Rail::Rankings => lobby.show_ranks(t, Instant::now()),
            Rail::Tourneys => lobby.tourneys.list.is_some(),
            Rail::Matches | Rail::Chat => true,
        });
        let Some((key, command)) = rail_read(rail, t) else { return loaded };
        if !self.allow_read(key) {
            return loaded;
        }
        self.send(command);
        true
    }

    fn save_layout(&mut self, bounds: WindowBounds) {
        let size = bounds.get_bounds().size;
        self.layout.width = f32::from(size.width);
        self.layout.height = f32::from(size.height);
        self.layout.maximized = matches!(bounds, WindowBounds::Maximized(_));
        if let Some(db) = &self.db {
            let _ = self.layout.save(db);
        }
    }

    fn cycle_theme(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.theme = self.theme.next();
        ui::install(self.theme.is_light(window.appearance()), cx);
        if let Some(db) = &self.db
            && let Err(e) = self.theme.save(db)
        {
            note(&mut self.log, format!("could not save the theme: {e}"));
        }
        cx.notify();
    }

    fn render_topbar(&self, cx: &mut Context<Self>) -> Div {
        // Red LIVE belongs to games; the connection uses its own colors.
        let (label, detail, color) = match &self.session.screen {
            Screen::Login => ("OFFLINE", String::new(), ui::muted()),
            Screen::Connecting(c) => ("CONNECTING", c.label.clone(), ui::pending()),
            Screen::Connected(c) => ("ONLINE", c.label.clone(), ui::accent()),
            Screen::Reconnecting(r) => (
                "RECONNECTING",
                format!("retry {} in {} s", r.attempt, retry_delay(r.attempt).as_secs()),
                ui::pending(),
            ),
        };
        let me = self.session.connected().map(|(c, _)| c.chat.me().to_string());
        div()
            .h(px(ui::TOPBAR_H))
            .flex_none()
            .h_flex()
            .justify_between()
            .gap_4()
            .px_4()
            .border_b_1()
            .border_color(ui::line())
            .child(
                div()
                    .flex_none()
                    .h_flex()
                    .gap_2()
                    .child(ui::disc(Color::White, ui::DISC_SM))
                    .child(ui::disc(Color::Black, ui::DISC_SM))
                    .child(div().ml_1().font_weight(FontWeight::BOLD).child("GGS Othello Viewer")),
            )
            // A long server label gives way first; the title and the buttons keep their width.
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_flex()
                    .justify_end()
                    .gap_2()
                    .child(caption().min_w_0().truncate().child(detail))
                    .child(
                        div()
                            .h_flex()
                            .gap_1p5()
                            .mr_2()
                            .font_family(ui::MONO)
                            .text_xs()
                            .text_color(color)
                            .child(ui::dot(color))
                            .child(label),
                    )
                    // While connecting or reconnecting these stay, disabled, so the buttons do not shift.
                    .when(!matches!(self.session.screen, Screen::Login), |d| {
                        let online = me.is_some();
                        d.child(
                            ui::icon_button("help", ui::IconName::CircleQuestionMark, "Help", false, online)
                                .when(online, |b| {
                                    b.on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.open_help("help", cx)))
                                }),
                        )
                        .child(
                            ui::icon_button("my-games", ui::IconName::User, "My games", false, online).when_some(
                                me,
                                |b, me| {
                                    b.on_click(
                                        cx.listener(move |this, _: &ClickEvent, _, cx| this.open_games(me.clone(), cx)),
                                    )
                                },
                            ),
                        )
                    })
                    .child(
                        ui::icon_button("toggle-log", ui::IconName::ScrollText, "Log", self.show_log, true).on_click(
                            cx.listener(|this, _: &ClickEvent, _, cx| {
                                this.show_log = !this.show_log;
                                cx.notify();
                            }),
                        ),
                    )
                    .child(
                        ui::icon_button(
                            "theme",
                            self.theme.icon(),
                            format!("Theme: {}", self.theme.name()),
                            false,
                            true,
                        )
                        .on_click(cx.listener(|this, _: &ClickEvent, window, cx| this.cycle_theme(window, cx))),
                    )
                    // Spelled out and last: as an icon it read like the Log toggle beside it.
                    .when(!matches!(self.session.screen, Screen::Login), |d| {
                        d.child(
                            ui::button("disconnect", "Log out", false, true)
                                .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.disconnect(cx))),
                        )
                    }),
            )
    }

    /// The log, newest message on top with each message's lines in order.
    fn render_log(&self) -> impl IntoElement {
        let blocks = self.log.iter().enumerate().map(|(i, block)| {
            div()
                .py_0p5()
                .border_b_1()
                .border_color(ui::column())
                .when(block.lines.len() > 1, |d| d.text_color(ui::ink()))
                .child(output_text(("log-block", block.id), block.lines.join("\n"), 3_000_000 + i as u64, &self.focus))
        });
        div().size_full().min_h_0().v_flex().border_t_1().border_color(ui::line()).bg(ui::surface()).child(
            div()
                .id("log")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .px_3()
                .py_1()
                .text_xs()
                .font_family(ui::MONO)
                .text_color(ui::muted())
                .children(blocks),
        )
    }

    fn shows_details(&self) -> bool {
        let lobby = &self.session.lobby;
        self.sheet.is_none() && lobby.view == ViewMode::Focus && lobby.focused().is_some()
    }

    fn render_connected(&self, chat: &Chat, lobby: &Lobby, window: &Window, cx: &mut Context<Self>) -> Stateful<Div> {
        let now = Instant::now();
        let vp = window.viewport_size();
        let show_details = self.shows_details();
        let layout = if show_details { &self.focus_layout } else { &self.plain_layout };
        let width = f32::from(vp.width);
        let height = f32::from(vp.height) - ui::TOPBAR_H;
        // The details pane's room stays reserved without it, so switching views does not move the rail.
        let minima = ui::SIDE_MINIMA;
        let scale = (width / minima.iter().sum::<f32>()).min(1.);
        let [left_min, center_min, right_min] = minima.map(|v| (v * scale).floor());
        let left = self.layout.left.min(width - center_min - right_min).max(left_min);
        let right = if show_details { self.layout.right.min(width - center_min - left).max(right_min) } else { 0. };
        let room =
            self.center_size.map(|s| size(f32::from(s.width) - 36., f32::from(s.height) - 16.)).unwrap_or_else(|| {
                let body_h = if self.show_log { height - self.layout.log } else { height };
                size(width - left - right - 36., body_h - 52.)
            });
        let center = match (&self.sheet, lobby.view, lobby.focused()) {
            (Some(Sheet::Help(text)), _, _) => render_help(text.as_ref(), self.back_button(cx), cx).into_any_element(),
            (Some(Sheet::Games(login)), _, _) => self.render_games(login, lobby, cx).into_any_element(),
            (Some(Sheet::Tourney(id)), _, _) => self.render_tourney(id, lobby, cx).into_any_element(),
            (None, ViewMode::Synchro, _) => {
                let height = self.center_size.map_or(room.height, |s| f32::from(s.height));
                render_synchro(lobby, size(room.width, height), &self.board, cx).into_any_element()
            }
            (None, ViewMode::Focus, Some(g)) => self.render_focus(lobby, g, room, now, cx).into_any_element(),
            (None, ViewMode::Focus, None) => empty_state(match &lobby.focus {
                Some(id) => format!("Loading {id}..."),
                None => "Select a match to view its board.".to_string(),
            })
            .into_any_element(),
        };
        let show_view_tabs = self.sheet.is_none() && lobby.has_synchro_focus();
        let view_tabs = view_tabs("view", "view", lobby.view, cx, |this, mode, cx| {
            this.sheet = None;
            this.with_lobby(cx, |l| l.show_view(mode));
        });
        div().id("connected").relative().size_full().flex().child(
            h_resizable(if show_details { "focus-layout" } else { "plain-layout" })
                .with_state(layout)
                .on_resize(cx.listener(move |this, state: &Entity<ResizableState>, _, cx| {
                    let sizes = state.read(cx).sizes();
                    if let Some(left) = sizes.first() {
                        this.layout.left = f32::from(*left);
                    }
                    if let Some(right) = sizes.get(2) {
                        this.layout.right = f32::from(*right);
                    }
                    cx.notify()
                }))
                .child(
                    resizable_panel()
                        .size(px(left))
                        .size_range(px(left_min)..px(600.))
                        .flex_grow_0()
                        .flex_shrink_1()
                        .child(self.render_rail(chat, lobby, cx)),
                )
                .child(
                    resizable_panel().size_range(px(center_min)..Pixels::MAX).child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .v_flex()
                            // The row keeps its height without tabs, so picking a synchro match does not move the board.
                            // A page puts Back in its own title row instead.
                            .when(self.sheet.is_none(), |d| {
                                d.child(
                                    div()
                                        .h(px(ui::VIEW_TABS_H))
                                        .flex_none()
                                        .h_flex()
                                        .gap_2()
                                        .px_4()
                                        .pt_3()
                                        .children(self.back_button(cx))
                                        .when(show_view_tabs, |d| d.child(view_tabs)),
                                )
                            })
                            // The scrollbar overlays a still frame; inside the scrolled area it would scroll away.
                            .child(
                                div()
                                    .relative()
                                    .flex_1()
                                    .min_h_0()
                                    .child(
                                        div()
                                            .id("center")
                                            .debug_selector(|| "center".into())
                                            .relative()
                                            .size_full()
                                            .track_scroll(&self.center_scroll)
                                            .overflow_y_scroll()
                                            // A sideways swipe over the crosstable scrolls only the table.
                                            .restrict_scroll_to_axis()
                                            .on_prepaint(measure(cx.weak_entity(), |this: &mut Viewer, s| {
                                                this.center_size.replace(s) != Some(s)
                                            }))
                                            // Clicking the board area takes the keys back from a text field.
                                            .on_mouse_down(
                                                MouseButton::Left,
                                                cx.listener(|this, _: &MouseDownEvent, window, cx| {
                                                    window.focus(&this.focus, cx)
                                                }),
                                            )
                                            .child(center),
                                    )
                                    .vertical_scrollbar(&self.center_scroll),
                            ),
                    ),
                )
                .when(show_details, |d| {
                    d.child(
                        resizable_panel()
                            .size(px(right))
                            .size_range(px(right_min)..px(600.))
                            .flex_grow_0()
                            .flex_shrink_1()
                            .child(self.render_details(
                                lobby.focused(),
                                lobby.focused().and_then(|g| self.board.views.get(g).cursor),
                                cx,
                            )),
                    )
                }),
        )
    }

    fn render_focus(&self, lobby: &Lobby, game: &Game, room: Size<f32>, now: Instant, cx: &mut Context<Self>) -> Div {
        let view = self.board.views.get(game);
        let at = view.cursor;
        let board = game.board(at);
        let halves: Vec<&Game> = lobby.halves().collect();
        let center_h = self.center_size.map_or(room.height, |s| f32::from(s.height));
        let cell = cell_size(size(room.width, self.board.focus_fit.board_room(center_h)), board.size());
        let width = (board.size() as f32 * cell + ui::COORD + ui::FRAME).max(room.width.min(440.));
        let chips = halves.iter().enumerate().map(|(i, g)| {
            let id = g.id.clone();
            ui::button(("half", i), short_id(&g.id).to_string(), g.id == game.id, true).on_click(cx.listener(
                move |this, _: &ClickEvent, _, cx| {
                    if let Some(l) = this.session.lobby_mut() {
                        l.focus = Some(id.clone());
                        cx.notify();
                    }
                },
            ))
        });
        let stop = if game.source == GameSource::Archive { "Close" } else { "Stop watching" };
        let komi = game.komi.map(|k| format!(" · komi {}", ui::signed(k, Some(2)))).unwrap_or_default();
        let gid = game.id.clone();
        let copy_id = game.id.clone();
        let (following, follow) = self.engine_follow(game);
        div()
            .v_flex()
            .items_center()
            .gap_3()
            .px_4()
            .py_3()
            .child(
                div()
                    .w(px(width))
                    .h_flex()
                    .justify_between()
                    .flex_wrap()
                    .gap_2()
                    .child(
                        div()
                            .h_flex()
                            .gap_2()
                            // A synchro pair names its match; the board buttons name each half.
                            .child(div().font_weight(FontWeight::BOLD).child(format!(
                                "{} · {}{komi}",
                                short_id(if halves.len() > 1 { match_of(&game.id) } else { &game.id }),
                                game.game_type
                            )))
                            .child(state_tag(game))
                            .when(halves.len() > 1, |d| d.children(chips)),
                    )
                    .child(
                        div()
                            .h_flex()
                            .gap_1()
                            .child(
                                ui::icon_button("copy-ggf", ui::IconName::Copy, "Copy GGF", false, true).on_click(
                                    cx.listener(move |this, _: &ClickEvent, _, cx| this.copy_ggf(&copy_id, cx)),
                                ),
                            )
                            .child(
                                ui::icon_button("hints", ui::IconName::Lightbulb, "Hints", view.hints, true).on_click(
                                    cx.listener(|this, _: &ClickEvent, _, cx| {
                                        if let Some(game) = this.session.lobby_mut().and_then(|l| l.focused()) {
                                            let view = this.board.views.get_mut(game);
                                            view.hints = !view.hints;
                                        }
                                        cx.notify();
                                    }),
                                ),
                            )
                            .child(
                                ui::icon_button("flip", ui::IconName::FlipVertical2, "Flip", view.flipped, true)
                                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.flip(cx))),
                            )
                            .child(
                                ui::icon_button("search-follow", ui::IconName::Cpu, "Engine logs", following, true)
                                    .debug_selector(|| "search-follow".into())
                                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                        this.send(follow.clone());
                                        cx.notify();
                                    })),
                            )
                            .child(ui::icon_button("unwatch", ui::IconName::X, stop, false, true).on_click(
                                cx.listener(move |this, _: &ClickEvent, _, cx| {
                                    this.with_lobby(cx, |l| l.unwatch(&gid))
                                }),
                            )),
                    ),
            )
            .child(focus_position(
                game,
                view,
                now,
                cell,
                width,
                measure(cx.weak_entity(), |this: &mut Viewer, s| this.board.focus_fit.board(s)),
            ))
            .child(under_board(game, at, width, cx, Viewer::navigate))
            .when(following, |d| d.child(engine_logs(game, at, view.flipped, width)))
            .on_prepaint(measure(cx.weak_entity(), |this: &mut Viewer, s| this.board.focus_fit.column(s)))
    }
}

/// A game id without the `archive ` prefix, as the ARCHIVE tag and the board buttons show it.
fn short_id(id: &str) -> &str {
    id.rsplit(' ').next().unwrap_or(id)
}

/// Back names the page it returns to.
fn back_label(back: &Sheet) -> String {
    match back {
        Sheet::Games(login) => format!("< Games of {login}"),
        Sheet::Help(_) => "< Help".into(),
        Sheet::Tourney(id) => format!("< Tournament {id}"),
    }
}

fn unread_mark(u: Unread) -> &'static str {
    match u {
        Unread::No => "",
        Unread::Yes => " *",
        Unread::ForMe => " !",
    }
}

enum ChatPatch {
    Reset(String),
    Append(String),
}

/// How the chat document changes from `shown` to `conversation`. One persistent document owns logical
/// selection while its blocks are virtualized: idle redraws keep it, appends preserve selection endpoints.
fn chat_patch(shown: &(Option<ChatKey>, usize), conversation: Option<&Conversation>) -> Option<ChatPatch> {
    let (key, previous) = shown;
    let count = conversation.map_or(0, |c| c.lines.len());
    if key.as_ref() != conversation.map(|c| &c.key) || count < *previous {
        Some(ChatPatch::Reset(conversation.map_or_else(String::new, |c| c.lines.iter().map(chat_document).collect())))
    } else {
        conversation
            .filter(|_| count > *previous)
            .map(|c| ChatPatch::Append(c.lines[*previous..].iter().map(chat_document).collect()))
    }
}

/// The read a rail's list comes from, keyed for `allow_read`; the Chat rail has none.
fn rail_read(rail: Rail, t: RatingType) -> Option<(String, AppCommand)> {
    Some(match rail {
        Rail::Matches => ("match".into(), proto::tell("match").into()),
        Rail::Tourneys => ("tourneys".into(), td::tell("tourneys").into()),
        Rail::Users => (format!("who {}", t.key()), AppCommand::Who(t)),
        Rail::Rankings => (format!("top {}", t.key()), AppCommand::Top(t)),
        Rail::Chat => return None,
    })
}

/// Tournament `id`'s schedule and standings, keyed for `allow_read`; /td answers them with ERR 2 before it starts.
fn tourney_results_read(id: &str) -> (String, Vec<String>) {
    (format!("td-results {id}"), vec![td::tell(&format!("st {id}")), td::tell(&format!("rankings {id}"))])
}

/// Between zero and `TD_REFRESH_JITTER`, from the random keys each `RandomState` draws.
fn jitter() -> Duration {
    use std::hash::BuildHasher;
    let r = std::collections::hash_map::RandomState::new().hash_one(());
    TD_REFRESH_JITTER.mul_f64(r as f64 / u64::MAX as f64)
}

fn chat_document(s: &Said) -> String {
    let time = chrono::DateTime::<chrono::Local>::from(s.at).format("%H:%M").to_string();
    // A level-6 heading is small and muted, with more room above it than below, so each message stands apart.
    format!("###### {} {time}\n\n{}\n\n", literal_markdown(&s.line.from), literal_markdown(&s.line.lines.join("\n")))
}

/// Protocol text remains literal, including markup characters and leading whitespace.
fn literal_markdown(text: &str) -> String {
    let mut escaped = String::new();
    for c in text.chars() {
        match c {
            ' ' => escaped.push_str("&#32;"),
            '\t' => escaped.push_str("&#9;"),
            '\n' => escaped.push_str("<br>"),
            c => {
                if c.is_ascii_punctuation() {
                    escaped.push('\\');
                }
                escaped.push(c);
            }
        }
    }
    escaped
}

fn output_text(
    id: impl Into<ElementId>,
    text: impl Into<SharedString>,
    order: u64,
    focus: &FocusHandle,
) -> impl IntoElement {
    let focus = focus.clone();
    div()
        .id(id)
        .on_mouse_down(MouseButton::Left, move |_, window, cx| window.focus(&focus, cx))
        .child(SelectableText::new("text", text).document_order(order))
        .test_support()
}

fn empty_state(title: impl Into<SharedString>) -> Div {
    div().size_full().flex().items_center().justify_center().text_color(ui::muted()).child(title.into())
}

impl Viewer {
    fn render_details(&self, game: Option<&Game>, at: At, cx: &mut Context<Viewer>) -> Stateful<Div> {
        let rail = div()
            .id("details")
            .debug_selector(|| "details".into())
            .v_flex()
            .gap_4()
            .p_4()
            .w_full()
            .min_w_0()
            .h_full()
            .overflow_y_scroll()
            .bg(ui::column())
            .border_l_1()
            .border_color(ui::line());
        let Some(game) = game else {
            return rail.child(ui::header("MOVES")).child(ui::placeholder("No game focused."));
        };
        let k = at.unwrap_or(game.moves().len());
        let can_kibitz = self.session.lobby.can_kibitz(game);
        let observers: Vec<&str> = game.observers.iter().map(String::as_str).collect();
        let moves = div().v_flex().gap_2().size_full().min_h_0().pb_2().child(ui::header("MOVES")).child(render_moves(
            game,
            k,
            &self.board.moves_scroll,
            cx,
            Viewer::navigate,
        ));
        let kibitz = div()
            .v_flex()
            .gap_2()
            .size_full()
            .min_h_0()
            .pt_2()
            .child(
                div()
                    .debug_selector(|| "kibitz-controls".into())
                    .h_flex()
                    .justify_between()
                    .child(ui::header("KIBITZ"))
                    // Hidden rather than dropped, so the header keeps its height from game to game.
                    .child(
                        ui::button(
                            "kibitz-to",
                            if self.kibitz_observers { "to observers" } else { "to all" },
                            self.kibitz_observers,
                            true,
                        )
                        .map(|b| match can_kibitz {
                            true => b.on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                this.kibitz_observers = !this.kibitz_observers;
                                cx.notify();
                            })),
                            false => b.invisible(),
                        }),
                    ),
            )
            .when(!observers.is_empty(), |d| {
                d.child(
                    caption()
                        .h_flex()
                        .gap_1()
                        .child(Icon::new(ui::IconName::Eye).flex_none())
                        .child(div().min_w_0().truncate().child(observers.join(", "))),
                )
            })
            .child(
                div()
                    .id(SharedString::from(format!("text{}", game.id)))
                    .debug_selector(|| "kibitz-text".into())
                    .track_scroll(&self.kibitz_scroll)
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .text_xs()
                    .text_color(ui::muted())
                    .children(game.text.iter().enumerate().map(|(i, l)| {
                        div().mb_1().child(output_text(
                            ("kibitz-line", i),
                            l.clone(),
                            2_000_000 + i as u64,
                            &self.focus,
                        ))
                    })),
            )
            .when(can_kibitz, |d| {
                d.child(
                    div()
                        .w_full()
                        .debug_selector(|| "kibitz-input".into())
                        .child(Input::new(&self.kibitz_input).small()),
                )
            });
        // The board, the player panels and the move list show the position; the rail keeps the record and the talk.
        rail.child(
            // Preserve both panel minima in the rail's scrollable content.
            div().flex_1().min_h(px(ui::MOVES_MIN + ui::KIBITZ_MIN)).relative().child(
                v_resizable("moves-kibitz")
                    .with_state(&self.moves_layout)
                    .child(resizable_panel().size_range(px(ui::MOVES_MIN)..Pixels::MAX).child(moves))
                    .child(
                        resizable_panel()
                            .size(px(ui::KIBITZ_H))
                            // A smaller window shrinks both panels by the same ratio; the floor keeps two kibitz rows.
                            .size_range(px(ui::KIBITZ_MIN)..Pixels::MAX)
                            .flex_none()
                            .child(kibitz),
                    ),
            ),
        )
    }

    /// Whether both players' log channels are joined, and the `chann` command that toggles them.
    fn engine_follow(&self, game: &Game) -> (bool, String) {
        let mut channels = vec![search::channel(&game.black.name), search::channel(&game.white.name)];
        channels.dedup();
        let joined = |c: &String| match &self.session.screen {
            Screen::Connected(conn) => conn.chat.conversations.iter().any(|v| v.key == ChatKey::Channel(c.clone())),
            _ => false,
        };
        let following = channels.iter().all(joined);
        let command = match following {
            true => format!("chann - {}", channels.join(" ")),
            false => {
                format!("chann + {}", channels.iter().filter(|c| !joined(c)).cloned().collect::<Vec<_>>().join(" "))
            }
        };
        (following, command)
    }
}

impl Render for Viewer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if std::mem::take(&mut self.attention) && !window.is_window_active() {
            window.request_attention();
        }
        // A resizable group keeps the sizes it measured and stretches them with the window, and a group
        // shown again brings back sizes from another window size. Cleared, the groups lay out again
        // from `self.layout`, which already fits the window.
        let laid_out_for = (window.viewport_size(), self.shows_details());
        if self.laid_out_for.replace(laid_out_for) != Some(laid_out_for) {
            for state in [&self.focus_layout, &self.plain_layout, &self.log_layout] {
                state.update(cx, |s, _| s.clear());
            }
        }
        let conversation = match &self.session.screen {
            Screen::Connected(conn) => conn.chat.current(),
            _ => None,
        };
        match chat_patch(&self.chat_shown, conversation) {
            Some(ChatPatch::Reset(document)) => self.chat_text.update(cx, |state, cx| state.set_text(&document, cx)),
            Some(ChatPatch::Append(document)) => self.chat_text.update(cx, |state, cx| state.push_str(&document, cx)),
            None => {}
        }
        self.chat_shown = (conversation.map(|c| c.key.clone()), conversation.map_or(0, |c| c.lines.len()));
        let body = match &self.session.screen {
            Screen::Login => self.render_login(cx).into_any_element(),
            Screen::Connecting(_) | Screen::Reconnecting(_) => {
                let title = if matches!(self.session.screen, Screen::Reconnecting(_)) {
                    "Reconnecting..."
                } else {
                    "Connecting..."
                };
                empty_state(title)
                    .flex_col()
                    .gap_3()
                    .when_some(self.log.front(), |d, b| d.child(caption().child(b.lines.join(" "))))
                    .child(
                        ui::button("cancel-connect", "Cancel", false, true)
                            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.disconnect(cx))),
                    )
                    .into_any_element()
            }
            Screen::Connected(conn) => {
                let lobby = &self.session.lobby;
                // Arrow keys reach the board only through a focused ancestor, so focus returns to the
                // window whenever no text field has it.
                if !self.focus.contains_focused(window, cx) {
                    window.focus(&self.focus, cx);
                }
                self.render_connected(&conn.chat, lobby, window, cx).into_any_element()
            }
        };
        let body = if self.show_log {
            let available = (f32::from(window.viewport_size().height) - ui::TOPBAR_H - 1.).max(0.);
            let log_min = 100_f32.min(available);
            let body_min = 240_f32.min(available - log_min);
            let log_height = self.layout.log.min(available - body_min).max(log_min);
            v_resizable("body-log")
                .with_state(&self.log_layout)
                .on_resize(cx.listener(|this, state: &Entity<ResizableState>, _, cx| {
                    if let Some(log) = state.read(cx).sizes().get(1) {
                        this.layout.log = f32::from(*log);
                    }
                    cx.notify()
                }))
                .child(
                    resizable_panel()
                        .size_range(px(body_min)..Pixels::MAX)
                        .child(div().size_full().min_h_0().child(body)),
                )
                .child(
                    resizable_panel()
                        .size(px(log_height))
                        .size_range(px(log_min)..Pixels::MAX)
                        .flex_grow_0()
                        .flex_shrink_1()
                        .child(self.render_log()),
                )
                .into_any_element()
        } else {
            body
        };
        div()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| {
                if !this.focus.is_focused(window) {
                    return;
                }
                let nav = match ev.keystroke.key.as_str() {
                    "left" => Nav::Prev,
                    "right" => Nav::Next,
                    "home" => Nav::First,
                    "end" => Nav::Live,
                    "f" => return this.flip(cx),
                    _ => return,
                };
                this.navigate(nav, cx);
            }))
            .size_full()
            .flex()
            .flex_col()
            .bg(ui::bg())
            .text_color(ui::ink())
            .child(self.render_topbar(cx))
            .child(div().flex_1().min_h_0().relative().child(body))
    }
}

#[cfg(test)]
mod ui_tests;

#[cfg(test)]
mod tests {
    use super::{
        LOG_CAP, MatchSettings, Recall, STABLE_LOGIN, TD_REFRESH_JITTER, eval_text, jitter, match_settings_rows,
        next_attempt, note, note_block, retry_delay,
    };
    use std::collections::VecDeque;
    use std::time::Duration;

    #[test]
    fn log_messages_keep_their_selection_ids_as_new_lines_arrive() {
        let mut log = VecDeque::new();
        note(&mut log, "first");
        let first_id = log[0].id;
        note_block(&mut log, Vec::new());
        for i in 1..=LOG_CAP {
            note(&mut log, format!("line {i}"));
            if i < LOG_CAP {
                assert_eq!(log[i].id, first_id);
                assert_eq!(log[i].lines, ["first"]);
            }
        }
        assert_eq!(log.len(), LOG_CAP);
        assert!(log.iter().all(|b| b.id != first_id));
        assert!(log.iter().zip(log.iter().skip(1)).all(|(a, b)| a.id > b.id));
    }

    #[test]
    fn match_settings_show_capacity_rules_and_unknown_values() {
        let mut s = MatchSettings {
            open: Some(2),
            playing: Some(1),
            rated: Some(true),
            accept: Some("size == 8 && rated".into()),
            decline: Some(String::new()),
            ..Default::default()
        };
        assert_eq!(
            match_settings_rows(&s),
            [
                ("Match slots", "1 playing / 2 allowed".into()),
                ("Rated setting", "Enabled".into()),
                ("Auto accept", "size == 8 && rated".into()),
                ("Auto decline", "Not configured".into()),
                ("Trust", "Not reported".into()),
            ]
        );
        s.playing = Some(2);
        assert_eq!(match_settings_rows(&s)[0].1, "2 playing / 2 allowed (full)");
        s.open = Some(0);
        assert_eq!(match_settings_rows(&s)[0].1, "Closed to new matches");
        s.playing = None;
        s.open = Some(2);
        assert_eq!(match_settings_rows(&s)[0].1, "Up to 2 simultaneous matches");
        s.accept = Some(String::new());
        s.rated = Some(false);
        let rows = match_settings_rows(&s);
        assert_eq!((rows[1].1.as_str(), rows[2].1.as_str()), ("Disabled", "Not configured"));
        assert!(match_settings_rows(&MatchSettings::default()).iter().all(|(_, value)| value == "Not reported"));
        s.trust = Some(true);
        assert_eq!(match_settings_rows(&s)[4], ("Trust", "Enabled (+)".into()));
        s.trust = Some(false);
        assert_eq!(match_settings_rows(&s)[4], ("Trust", "Disabled (-)".into()));
    }

    #[test]
    fn large_eval_labels_stay_compact() {
        assert_eq!(eval_text(196.), "+196.0");
        for e in [1e9, -1e9, f64::MAX, -f64::MAX] {
            assert!(eval_text(e).len() <= 8, "player and move evals must fit");
        }
    }

    #[test]
    fn recall_walks_back_and_returns_to_an_empty_line() {
        let mut r = Recall::default();
        assert_eq!(r.up(""), None, "nothing typed yet");
        for l in ["who", "help", "help"] {
            r.push(l.into());
        }
        assert_eq!(r.lines, ["who", "help"], "a repeat is kept once");
        assert_eq!(r.down(), None, "down does nothing while editing a new line");
        assert_eq!(r.up(""), Some("help"));
        assert_eq!(r.up("help"), Some("who"));
        assert_eq!(r.up("who"), Some("who"), "stops at the oldest");
        assert_eq!(r.down(), Some("help"));
        assert_eq!(r.down(), Some(""), "past the newest is a fresh line");
        assert_eq!(r.at, None);
        r.up("draft");
        r.push("finger".into());
        assert_eq!(r.up("unfinished"), Some("finger"), "sending starts again from the newest");
        assert_eq!(r.down(), Some("unfinished"), "Down restores the draft rather than losing it");
        assert_eq!(r.down(), None);
    }

    #[test]
    fn retries_back_off_to_a_minute() {
        let secs: Vec<u64> = (1..=8).map(|a| retry_delay(a).as_secs()).collect();
        assert_eq!(secs, [2, 4, 8, 16, 32, 60, 60, 60]);
        assert_eq!(retry_delay(u32::MAX), Duration::from_secs(60), "no overflow");
    }

    #[test]
    fn only_a_lost_login_or_a_failed_retry_is_retried() {
        let (stable, brief) = (Some(STABLE_LOGIN), Some(Duration::from_secs(1)));
        assert_eq!(next_attempt(stable, 0, true), Some(1), "a dropped login starts the backoff");
        assert_eq!(next_attempt(stable, 5, true), Some(1), "a lasting login restarts the backoff");
        assert_eq!(next_attempt(brief, 0, true), Some(1));
        assert_eq!(next_attempt(brief, 3, true), Some(4), "a login dropped right away keeps backing off");
        assert_eq!(next_attempt(None, 3, true), Some(4), "a failed retry backs off further");
        assert_eq!(next_attempt(None, u32::MAX, true), Some(u32::MAX));
        assert_eq!(next_attempt(None, 0, true), None, "the user's own connect failing goes back to login");
        assert_eq!(next_attempt(stable, 0, false), None, "a rejected login is not retried");
        assert_eq!(next_attempt(None, 2, false), None, "a rejected retry is not retried");
    }

    #[test]
    fn refresh_jitter_varies_within_its_window() {
        let draws: Vec<Duration> = (0..100).map(|_| jitter()).collect();
        assert!(draws.iter().all(|&d| d <= TD_REFRESH_JITTER));
        assert!(draws.iter().any(|&d| d != draws[0]), "the waits differ");
    }
}
