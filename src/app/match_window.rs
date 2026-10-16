use super::*;

#[path = "match_state.rs"]
mod state;
use state::{MatchContent, MatchState};

pub(super) struct MatchWindow {
    viewer: WeakEntity<Viewer>,
    state: MatchState,
    focus: FocusHandle,
    /// The scrolling area under the tabs, as last laid out.
    body: Option<Size<Pixels>>,
    board: BoardPane,
    _observe: Subscription,
}

impl MatchWindow {
    fn new(viewer: &Entity<Viewer>, mut state: MatchState, cx: &mut Context<Self>) -> Self {
        if let Some((_, lobby)) = viewer.read(cx).session.connected() {
            state.reconcile(lobby);
        }
        let observe = cx.observe(viewer, |this, viewer, cx| {
            if let Some((_, lobby)) = viewer.read(cx).session.connected() {
                this.state.reconcile(lobby);
            }
            cx.notify();
        });
        Self {
            viewer: viewer.downgrade(),
            state,
            focus: cx.focus_handle(),
            body: None,
            board: BoardPane::default(),
            _observe: observe,
        }
    }

    pub(super) fn clamp_view(&mut self, game: &Game, lobby: &Lobby) {
        // A join accepts its generation before another network event can arrive,
        // even when Viewer notifications or window frames are coalesced.
        self.state.reconcile(lobby);
        self.board.views.clamp(game);
    }

    /// Runs `f` on the selected game, if it is still held.
    fn with_selected<R>(&mut self, cx: &Context<Self>, f: impl FnOnce(&mut BoardPane, &Game) -> R) -> Option<R> {
        let id = self.state.selected_id();
        let viewer = self.viewer.upgrade()?;
        let (_, lobby) = viewer.read(cx).session.connected()?;
        let game = self.state.game(lobby, &id)?;
        Some(f(&mut self.board, game))
    }

    fn navigate(&mut self, nav: Nav, cx: &mut Context<Self>) {
        if self.state.mode != ViewMode::Focus {
            return;
        }
        self.with_selected(cx, |board, game| {
            board.views.navigate(game, nav);
            board.moves_scroll.scroll_to_item(record_row(game, board.views.get(game).cursor));
        });
        cx.notify();
    }

    fn toggle_engine_logs(&mut self, cx: &mut Context<Self>) {
        if let Some(viewer) = self.viewer.upgrade() {
            viewer.update(cx, |viewer, cx| viewer.toggle_engine_logs(cx));
        }
    }

    fn flip(&mut self, cx: &mut Context<Self>) {
        if self.state.mode != ViewMode::Focus {
            return;
        }
        self.with_selected(cx, |board, game| {
            let view = board.views.get_mut(game);
            view.flipped = !view.flipped;
        });
        cx.notify();
    }

    fn copy_ggf(&mut self, cx: &mut Context<Self>) {
        if let Some(ggf) = self.with_selected(cx, |_, game| game.to_ggf()) {
            cx.write_to_clipboard(ClipboardItem::new_string(ggf));
        }
    }

    fn render_halves(&self, cx: &Context<Self>) -> Div {
        div().h_flex().gap_1().children((0..2).map(|index| {
            ui::button(("child-half", index), format!("Board {}", index + 1), index == self.state.selected, true)
                .debug_selector(move || format!("child-half-{index}"))
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    this.state.selected = index;
                    cx.notify();
                }))
        }))
    }

    fn render_focus(&self, viewer: &Viewer, game: &Game, room: Size<f32>, cx: &Context<Self>) -> Div {
        let view = self.board.views.get(game);
        let toggle = viewer.engine_logs_toggle(game);
        let following = toggle == Some(true);
        if !following {
            self.board.search_scroll.forget();
        }
        let sidebar = if room.width >= 760. { 220. } else { 0. };
        let board_room = size((room.width - sidebar - 32.).max(0.), room.height);
        let cell = cell_size(
            size(board_room.width, self.board.focus_fit.board_room(room.height)),
            game.board(view.cursor).size(),
        );
        let width =
            (game.board(view.cursor).size() as f32 * cell + ui::COORD + ui::FRAME).max(board_room.width.min(440.));
        let board = div()
            .v_flex()
            .items_center()
            .gap_3()
            .p_4()
            .debug_selector(|| "child-focus-card".into())
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
                            .child(
                                div().font_weight(FontWeight::BOLD).child(format!("{} · {}", game.id, game.game_type)),
                            )
                            .child(state_tag(game)),
                    )
                    .child(
                        div()
                            .h_flex()
                            .gap_1()
                            .child(
                                ui::icon_button("child-copy", ui::IconName::Copy, "Copy GGF", false, true)
                                    .debug_selector(|| "child-copy".into())
                                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.copy_ggf(cx))),
                            )
                            .child(
                                ui::icon_button("child-hints", ui::IconName::Lightbulb, "Hints", view.hints, true)
                                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                        this.with_selected(cx, |board, game| {
                                            let view = board.views.get_mut(game);
                                            view.hints = !view.hints;
                                        });
                                        cx.notify();
                                    })),
                            )
                            .child(
                                ui::icon_button("child-flip", ui::IconName::FlipVertical2, "Flip", view.flipped, true)
                                    .debug_selector(|| "child-flip".into())
                                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.flip(cx))),
                            )
                            .children(toggle.map(|on| {
                                ui::icon_button("child-search-follow", ui::IconName::Logs, "Engine logs", on, true)
                                    .debug_selector(|| "child-search-follow".into())
                                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.toggle_engine_logs(cx)))
                            })),
                    ),
            )
            .child(focus_position(
                game,
                view,
                Instant::now(),
                cell,
                width,
                measure(cx.weak_entity(), |this: &mut Self, s| this.board.focus_fit.board(s)),
            ))
            .child(under_board(game, view.cursor, width, cx, Self::navigate))
            .when(following, |d| {
                let (height, _) = engine_logs_h(room.height);
                let scroll = &self.board.search_scroll;
                let searches = &viewer.session.lobby.searches;
                d.child(engine_logs(
                    searches,
                    scroll,
                    game,
                    view.cursor,
                    view.flipped,
                    board_room.width.max(width),
                    height,
                ))
            })
            .on_prepaint(measure(cx.weak_entity(), |this: &mut Self, s| this.board.focus_fit.column(s)));
        let moves = div()
            .w(px(if sidebar > 0. { sidebar } else { room.width }))
            .h(px(room.height.max(240.)))
            .v_flex()
            .gap_2()
            .p_3()
            .child(ui::header("MOVES"))
            .child(render_moves(
                game,
                view.cursor.unwrap_or(game.moves().len()),
                &self.board.moves_scroll,
                cx,
                Self::navigate,
            ));
        div().flex().flex_wrap().justify_center().items_start().child(board).child(moves)
    }
}

impl Viewer {
    pub(super) fn open_archive_window(&mut self, key: &str, cx: &mut Context<Self>) {
        self.with_lobby(cx, |lobby| lobby.request_look(key));
        self.open_match_window(&crate::game::archive_id(key), false, cx);
    }

    pub(super) fn watch_in_window(&mut self, id: &str, cx: &mut Context<Self>) {
        self.with_lobby(cx, |lobby| lobby.request_watch(id));
        self.open_match_window(id, true, cx);
    }

    pub(super) fn open_match_window(&mut self, id: &str, live: bool, cx: &mut Context<Self>) {
        let Some((_, lobby)) = self.session.connected() else { return };
        let state = MatchState::open(id, live, lobby);
        let id = state.id.clone();
        let listed_players = lobby.matches.iter().find_map(|entry| match entry {
            MatchEntry::Summary(m) if match_of(&m.id) == id => Some(format!("{} vs {}", m.player1, m.player2)),
            _ => None,
        });
        let held_players = lobby
            .games
            .values()
            .find(|game| match_of(&game.id) == id)
            .map(|game| format!("{} vs {}", game.black.name, game.white.name));
        let players = if state.historical { held_players.or(listed_players) } else { listed_players.or(held_players) };
        let title =
            players.map_or_else(|| format!("GGS Othello · {id}"), |players| format!("{players} · {id} · GGS Othello"));
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                None,
                size(px(if state.synchro { 1120. } else { 900. }), px(800.)),
                cx,
            ))),
            titlebar: Some(TitlebarOptions { title: Some(title.clone().into()), ..Default::default() }),
            ..Default::default()
        };
        let viewer = cx.entity();
        cx.defer(move |cx| {
            let existing = viewer.read(cx).match_windows.get(&id).cloned();
            if let Some((handle, child)) = existing
                && handle
                    .update(cx, |_, window, _| {
                        window.set_window_title(&title);
                        window.activate_window();
                    })
                    .is_ok()
            {
                let _ = child.update(cx, |child, cx| {
                    if let Some((_, lobby)) = viewer.read(cx).session.connected()
                        && child.state.reopen(state, lobby)
                    {
                        child.board.views = GameViews::default();
                    }
                    cx.notify();
                });
                return;
            }
            let result = gpui_kit::open_window(options, cx, |_, cx| cx.new(|cx| MatchWindow::new(&viewer, state, cx)));
            viewer.update(cx, |viewer, cx| {
                match result {
                    Ok((handle, child)) => {
                        viewer.match_windows.insert(id, (handle, child.downgrade()));
                    }
                    Err(_) => {
                        viewer.match_windows.remove(&id);
                        note(&mut viewer.log, "Could not open the match window.");
                        viewer.stop_unused_watches(Some(&id), cx);
                    }
                }
                cx.notify();
            });
        });
    }
}

impl Render for MatchWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let content = match self.viewer.upgrade() {
            None => empty_state("The viewer was closed."),
            Some(viewer) => match &viewer.read(cx).session.screen {
                Screen::Reconnecting(_) | Screen::Connecting(_) => {
                    empty_state("Reconnecting...").debug_selector(|| "match-reconnecting".into())
                }
                Screen::Login => empty_state("Disconnected.").debug_selector(|| "match-disconnected".into()),
                Screen::Connected(_) => {
                    let lobby = &viewer.read(cx).session.lobby;
                    match self.state.content {
                        MatchContent::ArchiveLoading => {
                            empty_state("Loading archived game...").debug_selector(|| "match-archive-loading".into())
                        }
                        MatchContent::ArchiveUnavailable => {
                            empty_state("This game is unavailable in the archive.").debug_selector(|| "match-archive-unavailable".into())
                        }
                        MatchContent::Stopped => {
                            empty_state("This match is no longer watched. Use the window button in the match list to watch it again.").debug_selector(|| "match-stopped".into())
                        }
                        MatchContent::Replaced => {
                            empty_state("This match has ended. Use the window button in the match list to open the current match.").debug_selector(|| "match-replaced".into())
                        }
                        MatchContent::Boards => {
                            let games = self.state.games(lobby);
                            let viewport = window.viewport_size();
                            let body = self.body.unwrap_or(size(
                                viewport.width,
                                viewport.height - px(if self.state.synchro { 44. } else { 0. }),
                            ));
                            let room = size((f32::from(body.width) - 32.).max(0.), f32::from(body.height).max(0.));
                            let body = if self.state.mode == ViewMode::Synchro {
                                let scroll = &self.board.search_scroll;
                                let (room, logs) = viewer.read(cx).synchro_logs(&lobby.searches, &games, scroll, room);
                                div().v_flex().child(render_match_boards(
                                    &self.state.id,
                                    &games,
                                    room,
                                    &self.board,
                                    cx,
                                    |w| &mut w.board,
                                    |w, game, cx| {
                                        w.state.selected = usize::from(game.ends_with(".1"));
                                        w.state.mode = ViewMode::Focus;
                                        cx.notify();
                                    },
                                ))
                                .children(logs)
                            } else if let Some(game) = games[self.state.selected] {
                                self.render_focus(viewer.read(cx), game, room, cx)
                            } else {
                                empty_state("Loading this board...").debug_selector(|| "match-live-loading".into())
                            };
                            let body = div()
                                .id("match-body")
                                .flex_1()
                                .min_h_0()
                                .overflow_y_scroll()
                                .on_prepaint(measure(cx.weak_entity(), |this: &mut Self, s| {
                                    this.body.replace(s) != Some(s)
                                }))
                                .child(body);
                            div()
                                .size_full()
                                .v_flex()
                                .when(self.state.synchro, |d| {
                                    d.child(
                                        div()
                                            .h_flex()
                                            .flex_wrap()
                                            .gap_2()
                                            .px_4()
                                            .pt_3()
                                            .child(view_tabs("child-view", "child", self.state.mode, cx, |this, mode, cx| {
                                                this.state.mode = mode;
                                                cx.notify();
                                            }))
                                            .when(self.state.mode == ViewMode::Focus, |d| {
                                                d.child(self.render_halves(cx))
                                            })
                                            .when(self.state.mode == ViewMode::Synchro, |d| {
                                                let games = self.state.games(lobby);
                                                let Some(game) = games.iter().flatten().next() else { return d };
                                                let Some(on) = viewer.read(cx).engine_logs_toggle(game) else { return d };
                                                d.child(div().flex_1()).child(
                                                    ui::icon_button(
                                                        "child-synchro-search-follow",
                                                        ui::IconName::Logs,
                                                        "Engine logs",
                                                        on,
                                                        true,
                                                    )
                                                    .debug_selector(|| "child-synchro-search-follow".into())
                                                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                                        this.toggle_engine_logs(cx)
                                                    })),
                                                )
                                            }),
                                    )
                                })
                                .child(body)
                        }
                    }
                }
            },
        };
        if window.focused(cx).is_none() {
            window.focus(&self.focus, cx);
        }
        div()
            .id("match-window")
            .debug_selector(|| "match-window".into())
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, _, cx| match ev.keystroke.key.as_str() {
                "left" => this.navigate(Nav::Prev, cx),
                "right" => this.navigate(Nav::Next, cx),
                "home" => this.navigate(Nav::First, cx),
                "end" => this.navigate(Nav::Live, cx),
                "f" => this.flip(cx),
                _ => (),
            }))
            .size_full()
            .bg(ui::bg())
            .text_color(ui::ink())
            .child(content)
    }
}
