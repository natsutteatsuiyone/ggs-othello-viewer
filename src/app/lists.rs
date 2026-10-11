use super::*;

impl Viewer {
    pub(super) fn render_games(&self, login: &str, lobby: &Lobby, cx: &mut Context<Self>) -> Div {
        let section = |title: &str, note: &str, rows: Option<&[Archived]>, cx: &mut Context<Self>| {
            let body = match rows {
                None => ui::placeholder(ui::LOADING).into_any_element(),
                Some([]) => ui::placeholder("No games.").into_any_element(),
                Some(rows) => div()
                    .v_flex()
                    .gap_1()
                    .children(rows.iter().rev().map(|a| archived_row(title, a, login, cx)))
                    .into_any_element(),
            };
            div()
                .v_flex()
                .gap_2()
                .child(
                    div()
                        .h_flex()
                        .gap_2()
                        .child(ui::header(title))
                        .child(div().text_xs().text_color(ui::muted()).child(note.to_string())),
                )
                .child(body)
        };
        div()
            .v_flex()
            .gap_4()
            .max_w(px(ui::PAGE_MAX_W))
            .px_6()
            .py_4()
            .child(
                div()
                    .h_flex()
                    .justify_between()
                    .child(
                        div()
                            .h_flex()
                            .gap_3()
                            .children(self.back_button(cx))
                            .child(div().font_weight(FontWeight::BOLD).child(format!("Games of {login}"))),
                    )
                    .child(
                        div()
                            .h_flex()
                            .gap_1()
                            // Watch comes and goes at the left end, so the other buttons stay put.
                            .when_some(lobby.playing(login).map(str::to_string), |d, id| {
                                d.child(
                                    ui::icon_button(
                                        "watch-user",
                                        ui::IconName::Eye,
                                        format!("Watch {id}"),
                                        false,
                                        true,
                                    )
                                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.watch(&id, cx))),
                                )
                            })
                            .child(
                                ui::icon_button("refresh-games", ui::IconName::RefreshCw, "Refresh", false, true)
                                    .debug_selector(|| "refresh-games".into())
                                    .on_click(cx.listener({
                                        let login = login.to_string();
                                        move |this, _: &ClickEvent, _, cx| this.open_games(login.clone(), true, cx)
                                    })),
                            )
                            .child(
                                ui::icon_button("tell-user", ui::IconName::MessageCircle, "Send a tell", false, true)
                                    .on_click(cx.listener({
                                        let login = login.to_string();
                                        move |this, _: &ClickEvent, _, cx| {
                                            this.open_chat(ChatKey::User(login.clone()), cx)
                                        }
                                    })),
                            )
                            .child(close_sheet(cx)),
                    ),
            )
            .child(
                div()
                    .h_flex()
                    .flex_wrap()
                    .items_start()
                    .gap_x_8()
                    .gap_y_4()
                    .child(render_profile(lobby.profiles.get(login)).flex_1().min_w(px(240.)))
                    .child(render_match_settings(lobby.match_settings.get(login)).flex_1().min_w(px(320.))),
            )
            .child(section("FINISHED", "Rated matches only.", lobby.history.get(login).map(Vec::as_slice), cx))
            .child(section("ADJOURNED", "", lobby.stored.get(login).map(Vec::as_slice), cx))
    }

    pub(super) fn render_tourney(&self, id: &str, lobby: &Lobby, cx: &mut Context<Self>) -> Div {
        let t = &lobby.tourneys;
        let now = Instant::now();
        let started = t.started(id);
        let schedule = t.schedules.get(id).map_or(&[][..], Vec::as_slice);
        let pending = |loaded: bool, none: &'static str| {
            ui::placeholder(if !started {
                "Not started yet."
            } else if loaded {
                none
            } else {
                ui::LOADING
            })
        };
        let about = match t.info(id) {
            None => ui::placeholder(ui::LOADING),
            Some(info) => {
                let style = match info.style {
                    td::Style::RoundRobin => "round robin",
                    td::Style::Swiss => "swiss",
                };
                div()
                    .v_flex()
                    .gap_0p5()
                    .text_sm()
                    .child(dotted([
                        style.to_string(),
                        info.game_type.to_string(),
                        info.clock.to_string(),
                        // The status line below says which round is on.
                        if info.rounds == 1 { "1 round".into() } else { format!("{} rounds", info.rounds) },
                    ]))
                    .child(div().font_weight(FontWeight::SEMIBOLD).child(t.status(id, now).unwrap_or_default()))
            }
        };
        let standings =
            if t.players(id).is_empty() { ui::placeholder(ui::LOADING) } else { standings_table(t, id, schedule, cx) };
        let rounds = match t.schedules.get(id) {
            Some(rows) if !rows.is_empty() => {
                div().v_flex().gap_3().children(rows.chunk_by(|a, b| a.round == b.round).map(|round| {
                    div()
                        .v_flex()
                        .gap_1()
                        .child(caption().child(format!("Round {}", round[0].round)))
                        // Fixed-width cards wrap into columns, so a big round stays short.
                        .child(
                            div()
                                .h_flex()
                                .flex_wrap()
                                .gap_1()
                                .children(round.iter().map(|p| pairing_row(p, lobby, cx))),
                        )
                }))
            }
            rows => pending(rows.is_some(), "No rounds."),
        };
        let refresh = id.to_string();
        div()
            .v_flex()
            .gap_4()
            .max_w(px(ui::PAGE_MAX_W))
            .px_6()
            .py_4()
            .child(
                div()
                    .h_flex()
                    .justify_between()
                    .child(
                        div()
                            .h_flex()
                            .gap_3()
                            .children(self.back_button(cx))
                            .child(div().font_weight(FontWeight::BOLD).child(format!("Tournament {id}"))),
                    )
                    .child(
                        div()
                            .h_flex()
                            .gap_1()
                            .child(
                                ui::icon_button("refresh-tourney", ui::IconName::RefreshCw, "Refresh", false, true)
                                    .debug_selector(|| "refresh-tourney".into())
                                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                        this.open_tourney(refresh.clone(), cx)
                                    })),
                            )
                            .child(close_sheet(cx)),
                    ),
            )
            .when(!tile_matches(lobby, id).is_empty(), |d| d.child(live_tiles(lobby, id, now, cx)))
            .child(about)
            .child(div().v_flex().gap_2().child(ui::header("STANDINGS")).child(standings))
            .child(div().v_flex().gap_2().child(ui::header("ROUNDS")).child(rounds))
    }

    pub(super) fn render_rail(&self, chat: &Chat, lobby: &Lobby, cx: &mut Context<Self>) -> Stateful<Div> {
        // An empty list says so only once its reply is in.
        let loading = match self.rail {
            Rail::Users => lobby.users_loading(Instant::now()),
            Rail::Rankings => lobby.ranks_loading(Instant::now()),
            Rail::Tourneys => lobby.tourneys.list.is_none(),
            Rail::Matches | Rail::Chat => false,
        };
        let empty = match self.rail {
            Rail::Matches => lobby.visible_matches().next().is_none(),
            Rail::Users => lobby.users().is_empty(),
            Rail::Rankings => lobby.ranks().is_empty(),
            Rail::Tourneys => lobby.tourneys.rows().is_empty(),
            Rail::Chat => false,
        };
        let types = [RatingType::Eight, RatingType::EightRandom].map(|t| {
            ui::button(t.key(), t.key(), self.rating_type == t, true)
                .debug_selector(move || format!("rating-type-{}", t.key()))
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    let previous = this.rating_type;
                    this.rating_type = t;
                    if !this.refresh_rail(false, cx) {
                        this.rating_type = previous;
                    }
                }))
        });
        let list = div()
            .id("rail-list")
            .debug_selector(|| "rail-list".into())
            .flex_1()
            .min_h_0()
            .v_flex()
            .gap_2()
            .px_4()
            .py_3()
            .when(self.rail != Rail::Chat, |d| d.overflow_y_scroll());
        let none = |text: &'static str| ui::placeholder(text).py_4();
        let list = match self.rail {
            _ if loading => list.child(none(ui::LOADING)),
            Rail::Matches => list
                .children(lobby.visible_matches().enumerate().map(|(i, e)| match_row(lobby, i, e, cx)))
                .when(empty, |d| d.child(none("No running matches.")))
                .when(!lobby.finished.is_empty(), |d| d.child(div().pt_2().child(ui::header("FINISHED"))))
                .children(lobby.finished.iter().rev().enumerate().map(|(i, f)| finished_row(lobby, i, f, cx))),
            Rail::Users => list
                .children(lobby.users().iter().enumerate().map(|(i, u)| user_row(i, u, cx)))
                .when(empty, |d| d.child(none("No users online."))),
            Rail::Rankings => list
                .children(lobby.ranks().iter().map(|r| rank_row(r, cx)))
                .when(empty, |d| d.child(none("No ranked players."))),
            Rail::Tourneys => list
                .when_some(lobby.tourneys.list.as_ref(), |d, rows| {
                    d.children(rows.iter().enumerate().map(|(i, s)| tourney_row(i, s, &lobby.tourneys, cx)))
                })
                .when(empty, |d| d.child(none("No tournaments."))),
            Rail::Chat => match chat.current() {
                Some(c) if !c.lines.is_empty() => {
                    list.child(TextView::new(&self.chat_text).scrollable(true).size_full().text_sm().style(
                        TextViewStyle::default().with_foreground(ui::ink().into()).with_paragraph_gap(rems(0.25)),
                    ))
                }
                _ => list.child(none("No messages.")),
            },
        };
        let conversations = chat.conversations.iter().enumerate().map(|(i, c)| {
            let key = c.key.clone();
            let label = match &c.key {
                ChatKey::User(u) => u.clone(),
                k => k.label(),
            };
            ui::button(("conversation", i), format!("{label}{}", unread_mark(c.unread)), c.key == chat.selected, true)
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.open_chat(key.clone(), cx)))
        });
        div()
            .id("left-rail")
            .debug_selector(|| "left-rail".into())
            .v_flex()
            .w_full()
            .min_w_0()
            .flex_none()
            .h_full()
            .bg(ui::column())
            .border_r_1()
            .border_color(ui::line())
            .child(
                div()
                    .h_flex()
                    .gap_1()
                    .px_4()
                    .pt_4()
                    .pb_2()
                    .child(
                        TabBar::new("rail")
                            .flex_1()
                            .segmented()
                            .xsmall()
                            .selected_index(RAILS.iter().position(|(r, _)| *r == self.rail).unwrap_or(0))
                            .on_click(cx.listener(|this, ix: &usize, _, cx| {
                                this.rail = RAILS[*ix].0;
                                this.refresh_rail(false, cx);
                            }))
                            .children(RAILS.iter().enumerate().map(|(i, &(rail, label))| {
                                let mark = if rail == Rail::Chat { unread_mark(chat.unread()) } else { "" };
                                Tab::new()
                                    .cursor_pointer()
                                    .flex_1()
                                    .label(format!("{label}{mark}"))
                                    .debug_selector(move || format!("rail-tab-{i}"))
                            })),
                    )
                    // Chat has nothing to refresh; its button keeps the place so the tabs do not move.
                    .child(
                        ui::icon_button("refresh", ui::IconName::RefreshCw, "Refresh", false, true)
                            .debug_selector(|| "rail-refresh".into())
                            .map(|b| match self.rail {
                                Rail::Chat => b.invisible(),
                                _ => b.on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                    this.refresh_rail(true, cx);
                                })),
                            }),
                    ),
            )
            .when(matches!(self.rail, Rail::Users | Rail::Rankings), |d| {
                d.child(
                    div()
                        .h_flex()
                        .gap_1()
                        .px_4()
                        .flex_wrap()
                        .child(div().mr_1().text_xs().text_color(ui::muted()).child("Rating type"))
                        .children(types)
                        .when(self.rail == Rail::Rankings, |d| {
                            d.child(div().flex_1()).child(caption().child("Score = rating − 3.5 × RD"))
                        }),
                )
            })
            .when(self.rail == Rail::Chat, |d| {
                d.child(
                    div()
                        .h_flex()
                        .flex_wrap()
                        .gap_1()
                        .px_4()
                        .children(conversations)
                        .child(div().flex_1())
                        .when_some(
                            match &chat.selected {
                                ChatKey::Channel(c) if c != DEFAULT_CHANNEL => Some(c.clone()),
                                _ => None,
                            },
                            |d, c| {
                                d.child(
                                    ui::icon_button("leave", ui::IconName::LogOut, "Leave channel", false, true)
                                        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                            this.send(format!("chann - {c}"));
                                            cx.notify();
                                        })),
                                )
                            },
                        )
                        .when(matches!(chat.selected, ChatKey::User(_)), |d| {
                            let key = chat.selected.clone();
                            d.child(ui::icon_button("close-chat", ui::IconName::X, "Close", false, true).on_click(
                                cx.listener(move |this, _: &ClickEvent, _, cx| {
                                    if let Screen::Connected(conn) = &mut this.session.screen {
                                        conn.chat.close(&key);
                                    }
                                    cx.notify();
                                }),
                            ))
                        }),
                )
                .child(div().px_4().pt_2().child(Input::new(&self.channel_input).small()))
            })
            .child(list)
            .when(self.rail == Rail::Chat, |d| d.child(div().px_4().pb_4().child(Input::new(&self.chat_input).small())))
    }
}

pub(super) fn match_row(lobby: &Lobby, i: usize, entry: &MatchEntry, cx: &mut Context<Viewer>) -> Stateful<Div> {
    let id = entry.id().to_string();
    let window_id = id.clone();
    let watched = lobby.is_watched(&id);
    let focused = lobby.focus.as_deref().is_some_and(|f| f == id || (match_of(f) == id && match_of(f) != f));
    let (title, sub) = match entry {
        MatchEntry::Summary(m) => (
            format!("{} vs {}", m.player1, m.player2),
            caption().child(format!("{} · {} / {} · {}", m.id, m.rating1, m.rating2, m.game_type)),
        ),
        MatchEntry::Raw(raw) => (id.clone(), caption().child(raw.clone())),
    };
    row(("match", i))
        .v_flex()
        .gap_0p5()
        .py_2()
        // The dot marks a watched match; the border and fill mark the one on the board.
        .border_color(if focused { ui::accent() } else { ui::surface() })
        .bg(if focused { ui::cursor_bg() } else { ui::surface() })
        .cursor_pointer()
        .hover(|s| s.bg(ui::cursor_bg()))
        .child(
            div()
                .h_flex()
                .gap_2()
                // The dot follows the title, so titles line up with their captions and the finished rows.
                .child(div().flex_1().min_w_0().truncate().font_weight(FontWeight::SEMIBOLD).child(title))
                .child(ui::dot(ui::accent()).when(!watched, |d| d.invisible()))
                .child(window_button(format!("match-window-{i}")).on_click(cx.listener(
                    move |this, _: &ClickEvent, _, cx| {
                        cx.stop_propagation();
                        this.watch_in_window(&window_id, cx);
                    },
                ))),
        )
        .child(sub)
        .debug_selector(move || format!("match-row-{i}"))
        .on_click(cx.listener(move |this, event: &ClickEvent, _, cx| {
            this.watch(&id, cx);
            if event.click_count() == 2 {
                this.open_live_match_window(&id, cx);
            }
        }))
}

/// An ended match: clicking it shows the boards this session still holds, or loads a rated one from
/// the archive. An unrated match is not archived, so it has nothing to open once its boards are gone.
pub(super) fn finished_row(lobby: &Lobby, i: usize, ended: &FinishedMatch, cx: &mut Context<Viewer>) -> Stateful<Div> {
    let f = &ended.report;
    let m = &f.summary;
    let held = ended.is_held(lobby);
    let id = m.id.clone();
    let archive = f.archive.clone();
    let opens = held || archive.is_some();
    let window_id = id.clone();
    let window_archive = archive.clone();
    row(("finished", i))
        .v_flex()
        .gap_0p5()
        .py_2()
        .when(opens, |d| d.cursor_pointer().hover(|s| s.bg(ui::cursor_bg())))
        .child(
            div()
                .h_flex()
                .gap_2()
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_sm()
                        .text_color(ui::muted())
                        .child(format!("{} vs {}", m.player1, m.player2)),
                )
                .when(opens, |d| {
                    d.child(window_button(format!("finished-window-{i}")).on_click(cx.listener(
                        move |this, _: &ClickEvent, _, cx| {
                            cx.stop_propagation();
                            match (held, &window_archive) {
                                (false, Some(key)) => this.open_archive_window(key, cx),
                                _ => this.open_match_window(&window_id, cx),
                            }
                        },
                    )))
                }),
        )
        .child(
            div().h_flex().gap_1().child(caption().child(format!("{} · {} ·", m.id, m.game_type))).child(
                div()
                    .min_w_0()
                    .truncate()
                    .font_family(ui::MONO)
                    .text_xs()
                    .child(f.outcome.describe().pop().unwrap_or_default()),
            ),
        )
        .when(opens, |d| {
            d.on_click(cx.listener(move |this, _: &ClickEvent, _, cx| match (held, &archive) {
                (false, Some(key)) => this.look(key.clone(), cx),
                _ => {
                    this.leave_sheet();
                    this.with_lobby(cx, |l| {
                        l.show(&id);
                        Vec::new()
                    });
                }
            }))
        })
}

/// A `top` row; a click lists the player's games.
pub(super) fn rank_row(r: &Rank, cx: &mut Context<Viewer>) -> Stateful<Div> {
    let name = r.name.clone();
    row(SharedString::from(format!("rank{}", r.name)))
        .v_flex()
        .gap_0p5()
        .py_2()
        .bg(ui::surface())
        .cursor_pointer()
        .hover(|s| s.bg(ui::cursor_bg()))
        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.open_games(name.clone(), false, cx)))
        .child(
            div()
                .h_flex()
                .gap_2()
                .child(caption().w(px(22.)).child(r.place.to_string()))
                .child(div().flex_1().min_w_0().truncate().font_weight(FontWeight::SEMIBOLD).child(r.name.clone()))
                .child(
                    div()
                        .font_family(ui::MONO)
                        .text_lg()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(format!("{:.1}", r.rating - 3.5 * r.stdev)),
                ),
        )
        .child(
            div()
                .v_flex()
                .pl(px(30.))
                .font_family(ui::MONO)
                .text_xs()
                .text_color(ui::muted())
                .child(div().child(format!("rating {:.1} ±{:.1}", r.rating, r.stdev)))
                .child(div().child(format!("{}-{}-{} · avg {}", r.win, r.draw, r.loss, ui::signed(r.ascore, Some(1))))),
        )
}

/// A `tourneys` row; a click opens the tournament.
pub(super) fn tourney_row(i: usize, s: &td::Summary, t: &td::Tourneys, cx: &mut Context<Viewer>) -> Stateful<Div> {
    let id = s.id.clone();
    let progress = t.progress(s, Instant::now());
    row(("tourney", i))
        .v_flex()
        .gap_0p5()
        .py_2()
        .bg(ui::surface())
        .cursor_pointer()
        .hover(|s| s.bg(ui::cursor_bg()))
        .debug_selector(move || format!("tourney-row-{i}"))
        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.open_tourney(id.clone(), cx)))
        .child(
            div()
                .h_flex()
                .gap_2()
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(format!("Tournament {}", s.id)),
                )
                .when_some(progress, |d, p| d.child(div().text_xs().child(p))),
        )
        .child(caption().h_flex().gap_1().child(Icon::new(ui::IconName::Users).flex_none()).child(dotted([
            t.player_count(s).to_string(),
            s.game_type.to_string(),
            s.clock.to_string(),
        ])))
}

/// Items joined by dots on a line, wrapping only between items and leaving the dot at the line's end.
fn dotted<const N: usize>(items: [String; N]) -> Div {
    div().h_flex().flex_wrap().gap_x_1().children(
        items
            .into_iter()
            .enumerate()
            .map(|(i, item)| div().whitespace_nowrap().child(if i + 1 < N { format!("{item} ·") } else { item })),
    )
}

/// The running /os match of a pairing being played. The match line names no tournament, so a game is
/// found by its players among running tournament matches.
fn pairing_match(p: &td::Pairing, lobby: &Lobby) -> Option<String> {
    if p.result != td::Outcome::Playing {
        return None;
    }
    lobby.matches.iter().find_map(|e| match e {
        MatchEntry::Summary(m) if m.mode == "T" && m.player1 == p.black && Some(&m.player2) == p.white.as_ref() => {
            Some(m.id.clone())
        }
        _ => None,
    })
}

/// The most live boards a tournament shows, which bounds the watches it holds.
const TILES: usize = 8;

/// The known /os matches of tournament `id`'s games being played, in schedule order.
pub(super) fn live_matches(lobby: &Lobby, id: &str) -> Vec<String> {
    lobby.tourneys.schedules.get(id).into_iter().flatten().filter_map(|p| pairing_match(p, lobby)).collect()
}

/// The matches shown as tiles, and watched for them: the games of the round on show in schedule order,
/// the round being played or else the last with a result. A game that ends before the rest of its round
/// keeps its boards, and the round's boards stay until the next round's games replace them.
pub(super) fn tile_matches(lobby: &Lobby, id: &str) -> Vec<String> {
    let schedule = lobby.tourneys.schedules.get(id).map_or(&[][..], Vec::as_slice);
    let last = |on: fn(&td::Outcome) -> bool| schedule.iter().filter(|p| on(&p.result)).map(|p| p.round).max();
    let round = last(|o| *o == td::Outcome::Playing).or_else(|| last(|o| matches!(o, td::Outcome::Margin { .. })));
    let meets = |p: &td::Pairing, g: &Game| {
        let (a, b) = (&g.black.name, &g.white.name);
        p.white.as_ref().is_some_and(|w| (&p.black, w) == (a, b) || (&p.black, w) == (b, a))
    };
    let held = |p: &td::Pairing| {
        lobby
            .background
            .iter()
            .find(|m| lobby.games.values().any(|g| match_of(&g.id) == m.as_str() && meets(p, g)))
            .cloned()
    };
    schedule
        .iter()
        .filter(|p| Some(p.round) == round)
        .filter_map(|p| pairing_match(p, lobby).or_else(|| held(p)))
        .take(TILES)
        .collect()
}

fn live_tiles(lobby: &Lobby, id: &str, now: Instant, cx: &mut Context<Viewer>) -> Div {
    let live = live_matches(lobby, id).len();
    let more = live.saturating_sub(TILES);
    let all = tile_matches(lobby, id);
    div()
        .v_flex()
        .gap_2()
        .child(ui::header(if live > 0 { "LIVE" } else { "LAST ROUND" }))
        .child(
            div()
                .h_flex()
                .flex_wrap()
                .items_start()
                .gap_3()
                .children(all.into_iter().map(|m| live_tile(&m, lobby, now, cx))),
        )
        .when(more > 0, |d| {
            d.child(caption().debug_selector(|| "tourney-tiles-more".into()).child(format!("and {more} more")))
        })
}

/// The boards of match `m`, two for synchro; a click shows it on the main board, like the pairing's Watch button.
fn live_tile(m: &str, lobby: &Lobby, now: Instant, cx: &mut Context<Viewer>) -> Stateful<Div> {
    let mut games: Vec<&Game> = lobby.games.values().filter(|g| match_of(&g.id) == m).collect();
    games.sort_by(|a, b| a.id.cmp(&b.id));
    let selector = format!("tourney-tile-{m}");
    let id = m.to_string();
    let tile = div()
        .id(SharedString::from(selector.clone()))
        .debug_selector(move || selector.clone())
        .h_flex()
        .items_start()
        .gap_4()
        .p_3()
        .rounded(px(ui::RADIUS))
        .border_1()
        .border_color(ui::line())
        .bg(ui::surface())
        .cursor_pointer()
        .hover(|s| s.bg(ui::cursor_bg()))
        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.watch(&id, cx)));
    if games.is_empty() {
        return tile
            .w(px(226.))
            .h(px(120.))
            .items_center()
            .justify_center()
            .text_sm()
            .text_color(ui::muted())
            .child(format!("Loading {m}..."));
    }
    tile.children(games.into_iter().map(|g| tile_board(g, now)))
}

fn tile_board(game: &Game, now: Instant) -> Div {
    let board = game.board(None);
    // No coordinates on a tile, so the board takes their margin.
    let cell = ((176. + ui::COORD) / board.size() as f32).floor();
    let side = |color: Color, name: &str| {
        let to_move = game.shows_turn(None) && board.to_move() == color;
        div()
            .h_flex()
            .gap_2()
            .font_family(ui::MONO)
            .text_xs()
            .child(ui::disc(color, ui::DISC_SM))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_sm()
                    .when(to_move, |d| d.text_color(ui::accent()).font_weight(FontWeight::BOLD))
                    .child(name.to_string()),
            )
            .when_some(game.stated_eval(color, None), |d, e| {
                let selector = format!("tourney-tile-eval-{}-{}", game.id, ui::color_name(color));
                d.child(
                    div().debug_selector(move || selector.clone()).text_color(ui::eval_ink(color)).child(eval_text(e)),
                )
            })
            .child(
                div()
                    .text_color(if game.is_running(color, None) { ui::ink() } else { ui::muted() })
                    .child(game.clock_text(color, None, now)),
            )
    };
    let tile_selector = format!("tourney-tile-board-{}", game.id);
    div()
        .v_flex()
        .gap_2()
        .w(px(board.size() as f32 * cell + ui::FRAME))
        .child(side(Color::Black, &game.black.name))
        .child(
            render_board(game, None, BoardOpts { cell, hints: false, flipped: false, coords: false })
                .debug_selector(move || tile_selector.clone()),
        )
        .child(side(Color::White, &game.white.name))
}

/// The standings and the crosstable as one table in place order, one row per player. Place, movement
/// and name stay put; the numbers and the games scroll sideways beside them. A round robin heads its game
/// columns with places; a Swiss field meets only a few of its players, so its columns are the rounds.
/// Before the rankings come the rows follow finger's order and leave the numbers out.
fn standings_table(t: &td::Tourneys, id: &str, schedule: &[td::Pairing], cx: &mut Context<Viewer>) -> Div {
    const ROW_H: f32 = 28.;
    const HEAD_H: f32 = 22.;
    let players = t.players(id);
    let standings = t.standings.get(id).map_or(&[][..], Vec::as_slice);
    let moved = if t.finished(id) { Default::default() } else { td::movement(schedule) };
    let medals = !standings.is_empty() && td::decided(schedule) > 0;
    let swiss = t.info(id).is_some_and(|i| i.style == td::Style::Swiss);
    let rounds = schedule.iter().map(|p| p.round).chain(t.info(id).map(|i| i.rounds)).max().unwrap_or(0);
    let crosstable = td::crosstable(&players, schedule);
    let shared = td::shared_decided(&crosstable).filter(|_| !swiss);
    let round_table = td::rounds_table(&players, schedule, rounds);
    let games = !schedule.is_empty();
    let cell_w = px(76.);
    let row = || div().h(px(ROW_H)).flex_none().h_flex().border_t_1().border_color(ui::line());
    let fixed =
        div().flex_none().v_flex().child(div().h(px(HEAD_H))).children(players.iter().enumerate().map(|(i, login)| {
            let medal = ui::medal(i + 1).filter(|_| medals);
            let arrow = match moved.get(login).copied() {
                Some(n) if n > 0 => Some((format!("▲{n}"), ui::accent())),
                Some(n) if n < 0 => Some((format!("▼{}", -n), ui::live())),
                _ => None,
            };
            let name = login.clone();
            row()
                .id(("standing", i))
                .debug_selector(move || format!("standing-{i}"))
                .gap_2()
                .pl_1()
                .pr_3()
                .cursor_pointer()
                .hover(|s| s.bg(ui::cursor_bg()))
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.open_games(name.clone(), false, cx)))
                .child(
                    div()
                        .w(px(22.))
                        .flex()
                        .justify_center()
                        .text_color(ui::muted())
                        .when_some(medal, |d, c| {
                            d.debug_selector(move || format!("standing-medal-{i}"))
                                // As wide as a plain place, so the names line up.
                                .size(px(22.))
                                .rounded_full()
                                .bg(c)
                                .items_center()
                                .text_color(ui::bg())
                                .font_weight(FontWeight::BOLD)
                        })
                        .child((i + 1).to_string()),
                )
                .child(div().w(px(24.)).when_some(arrow, |d, (text, c)| {
                    d.debug_selector(move || format!("standing-move-{i}")).text_color(c).child(text)
                }))
                .child(div().w(px(120.)).truncate().text_sm().font_weight(FontWeight::SEMIBOLD).child(login.clone()))
        }));
    let stat = |w: f32| div().w(px(w)).flex_none().pr_2().text_right();
    let head = div()
        .h(px(HEAD_H))
        .flex_none()
        .h_flex()
        .text_color(ui::muted())
        .child(stat(48.).child("Pts"))
        .child(stat(64.).child("W-D-L"))
        .child(stat(64.).child("Avg"))
        .child(stat(72.).child("Tiebreak"))
        .when(games, |d| {
            d.child(div().w(px(12.))).children((1..=if swiss { rounds as usize } else { players.len() }).map(|n| {
                div().w(cell_w).flex_none().text_center().child(if swiss { format!("R{n}") } else { n.to_string() })
            }))
        });
    let rows = players.iter().enumerate().map(|(r, login)| {
        let s = standings.iter().find(|s| &s.login == login);
        row()
            .child(stat(48.).text_sm().font_weight(FontWeight::BOLD).children(s.map(|s| format!("{:.1}", s.points))))
            .child(stat(64.).children(s.map(|s| format!("{}-{}-{}", s.wins, s.draws, s.losses))))
            .child(stat(64.).children(s.map(|s| ui::signed(s.discs, Some(2)))))
            .child(stat(72.).text_color(ui::muted()).children(s.map(|s| format!("{:.4}", s.tiebreak))))
            .when(games && !swiss, |d| {
                d.child(div().w(px(12.))).children(crosstable[r].iter().enumerate().map(|(c, games)| {
                    let selector = format!("cross-{r}-{c}");
                    div()
                        .w(cell_w)
                        .h_full()
                        .flex_none()
                        .h_flex()
                        .justify_center()
                        .gap_1()
                        .when(r == c, |d| d.bg(ui::column()))
                        .when(!games.is_empty(), |d| d.debug_selector(move || selector.clone()))
                        .child(meeting_score(games, shared))
                }))
            })
            .when(games && swiss, |d| {
                d.child(div().w(px(12.))).children(round_table[r].iter().enumerate().map(|(k, cell)| {
                    let selector = format!("round-{r}-{k}");
                    div()
                        .w(cell_w)
                        .h_full()
                        .flex_none()
                        .h_flex()
                        .justify_center()
                        .gap_1()
                        .when(cell.is_some(), |d| d.debug_selector(move || selector.clone()))
                        .children(cell.map(round_cell))
                }))
            })
    });
    let scroll = div().debug_selector(|| "crosstable".into()).flex_1().min_w_0().child(
        div()
            .id("crosstable")
            // The horizontal scrollbar overlays the table's foot; the padding keeps it off the last row.
            .child(div().flex_none().v_flex().pb_4().child(head).children(rows))
            .overflow_x_scrollbar(),
    );
    div()
        .v_flex()
        .gap_1()
        .when_some(shared.filter(|&n| n > 1), |d, n| {
            d.child(caption().debug_selector(|| "cross-games".into()).child(format!("{n} games per pair")))
        })
        .child(div().h_flex().items_start().font_family(ui::MONO).text_xs().child(fixed).child(scroll))
}

/// Green for a win, red for a loss, muted for a draw, from the sign of a margin or a score above even.
fn sign_ink(x: f64) -> Rgba {
    if x > 0. {
        ui::accent()
    } else if x < 0. {
        ui::live()
    } else {
        ui::muted()
    }
}

/// A Swiss round from the row player's side: the opponent's place and the disc margin, or a bye.
/// Both parts keep a fixed width and sit flush right, so a column lines up digit for digit.
fn round_cell(cell: td::RoundCell) -> Div {
    let (opponent, score, color) = match cell {
        td::RoundCell::Bye => (String::new(), "bye".into(), ui::muted()),
        td::RoundCell::Met { opponent, game } => {
            let (score, color) = match game {
                td::CellGame::Margin(m) => (ui::signed(m, None), sign_ink(m)),
                td::CellGame::Playing => ("…".into(), ui::pending()),
                td::CellGame::Unstarted => (String::new(), ui::muted()),
            };
            ((opponent + 1).to_string(), score, color)
        }
    };
    div()
        .h_flex()
        .gap_1()
        .child(div().w(px(22.)).text_right().text_color(ui::muted()).child(opponent))
        .child(div().w(px(28.)).text_right().text_color(color).child(score))
}

/// The row player's points over the decided games of a meeting, `7.5/12`, and `…` while one is played.
/// The `/12` goes when every meeting shares it.
fn meeting_score(games: &[td::CellGame], shared: Option<usize>) -> Div {
    let margins: Vec<f64> =
        games.iter().filter_map(|g| if let td::CellGame::Margin(m) = g { Some(*m) } else { None }).collect();
    let points: f64 = margins
        .iter()
        .map(|&m| {
            if m > 0. {
                1.
            } else if m == 0. {
                0.5
            } else {
                0.
            }
        })
        .sum();
    let even = margins.len() as f64 / 2.;
    let color = sign_ink(points - even);
    let playing = games.iter().any(|g| matches!(g, td::CellGame::Playing));
    // One decimal throughout, so 1.0, 0.5 and 0.0 are as wide and a column lines up.
    let score =
        if shared == Some(margins.len()) { format!("{points:.1}") } else { format!("{points:.1}/{}", margins.len()) };
    div()
        .h_flex()
        .gap_1()
        .when(!margins.is_empty(), |d| d.child(div().text_color(color).child(score)))
        .when(playing, |d| d.child(div().text_color(ui::pending()).child("…")))
}

/// A pairing card's natural height, fixed so layout need not measure every card's text on each frame,
/// which made a long schedule scroll at a few frames a second.
// ponytail: every round still lays out each frame, ~50 ms at 30 players round robin; skip the rounds off
// screen if fields that big turn up.
const PAIRING_H: f32 = 61.;

/// A pairing as a card of two lines, black over white, each with its disc, name and the margin from
/// that player's side, like a live tile. The winner is bold, the margin green or red. A game being
/// played shows its board when clicked, like the tiles.
fn pairing_row(p: &td::Pairing, lobby: &Lobby, cx: &mut Context<Viewer>) -> Stateful<Div> {
    let margin = match p.result {
        td::Outcome::Margin { black, .. } => Some(black),
        _ => None,
    };
    let ending = match p.result {
        td::Outcome::Margin { ending: td::Ending::Resigned, .. } => Some("resigned"),
        td::Outcome::Margin { ending: td::Ending::Timeout, .. } => Some("on time"),
        _ => None,
    };
    let playing = p.result == td::Outcome::Playing;
    let line = |color: Color, login: Option<&str>| {
        // The margin from this line's side: black's as /td gives it, white's negated.
        let mine = margin.map(|m| if color == Color::Black { m } else { 0. - m });
        let (score, score_color) = match (&p.result, mine) {
            (_, Some(m)) => (ui::signed(m, None), sign_ink(m)),
            (td::Outcome::Playing, _) => ("…".into(), ui::pending()),
            (td::Outcome::Bye, _) if color == Color::Black => ("bye".into(), ui::muted()),
            _ => (String::new(), ui::muted()),
        };
        let won = mine.is_some_and(|m| m > 0.);
        let lost = mine.is_some_and(|m| m < 0.);
        div()
            .h_flex()
            .gap_2()
            // A rest or a bye has no white line, but keeps its height so the cards stay even.
            .when(login.is_none(), |d| d.invisible())
            .child(ui::disc(color, ui::DISC_SM))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .when(won, |d| d.font_weight(FontWeight::BOLD))
                    .when(lost, |d| d.text_color(ui::muted()))
                    .child(login.unwrap_or("-").to_string()),
            )
            .when_some(ending.filter(|_| lost), |d, e| d.child(div().text_xs().text_color(ui::muted()).child(e)))
            .child(div().w(px(32.)).flex_none().text_right().text_color(score_color).child(score))
    };
    let white = p.white.as_deref().filter(|_| p.result != td::Outcome::Bye);
    let game = pairing_match(p, lobby);
    div()
        .id(SharedString::from(format!("pairing-{}-{}", p.round, p.board)))
        .debug_selector({
            let selector = format!("pairing-{}-{}", p.round, p.board);
            move || selector.clone()
        })
        .w(px(220.))
        .h(px(PAIRING_H))
        .h_flex()
        .items_start()
        .gap_2()
        .px_3()
        .py_1p5()
        .rounded(px(ui::RADIUS_ROW))
        .border_1()
        .border_color(if playing { ui::pending() } else { ui::surface() })
        .bg(ui::surface())
        .font_family(ui::MONO)
        .text_sm()
        .child(div().w(px(16.)).flex_none().text_xs().text_color(ui::muted()).child(p.board.to_string()))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .v_flex()
                .gap_0p5()
                .child(line(Color::Black, Some(&p.black)))
                .child(line(Color::White, white)),
        )
        .when_some(game, |d, id| {
            let selector = format!("tourney-watch-{}-{}", p.round, p.board);
            d.debug_selector(move || selector.clone())
                .cursor_pointer()
                .hover(|s| s.bg(ui::cursor_bg()))
                .tooltip({
                    let tip = format!("Watch {id}");
                    move |window, cx| gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(window, cx)
                })
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.watch(&id, cx)))
        })
}

pub(super) fn close_sheet(cx: &mut Context<Viewer>) -> Stateful<Div> {
    ui::icon_button("close-sheet", ui::IconName::X, "Close", false, true).on_click(cx.listener(
        |this, _: &ClickEvent, _, cx| {
            this.sheet = None;
            cx.notify();
        },
    ))
}

/// A help file and the buttons that ask for the others.
pub(super) fn render_help(text: Option<&HelpText>, back: Option<Stateful<Div>>, cx: &mut Context<Viewer>) -> Div {
    let close = close_sheet(cx);
    let files = HELP_FILES.iter().enumerate().map(|(i, (label, cmd))| {
        ui::button(("help-file", i), *label, false, true)
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.open_help(cmd, cx)))
    });
    div()
        .v_flex()
        .gap_3()
        .px_6()
        .py_4()
        .child(
            div().h_flex().justify_between().child(div().h_flex().gap_1().children(back).children(files)).child(close),
        )
        .child(match text {
            None => ui::placeholder(ui::LOADING),
            Some(h) => {
                ui::boxed()
                    .p_4()
                    .gap_0p5()
                    .font_family(ui::MONO)
                    .text_xs()
                    .child(div().pb_2().font_weight(FontWeight::BOLD).child(h.title.clone()))
                    .children(h.lines.iter().map(|l| {
                        div().whitespace_nowrap().child(if l.is_empty() { " ".to_string() } else { l.clone() })
                    }))
            }
        })
}

/// Name, info, whether the login is on, and its channels, as the server's `finger` gives them.
pub(super) fn render_profile(profile: Option<&Option<Profile>>) -> Div {
    let body = div().v_flex().gap_1().text_sm();
    let body = match profile {
        None => body.text_color(ui::muted()).child(ui::LOADING),
        Some(None) => body.text_color(ui::muted()).child("The server does not know this login."),
        Some(Some(p)) => {
            let seen = match &p.seen {
                Seen::On { since, idle: Some(idle) } => {
                    format!("on since {since} · idle {}", idle.replace(", on line :", " · on for"))
                }
                Seen::On { since, idle: None } => format!("on since {since}"),
                Seen::Off { last } => format!("last on {last}"),
                Seen::Unknown => String::new(),
            };
            body.when(!p.name.is_empty(), |d| d.child(div().font_weight(FontWeight::SEMIBOLD).child(p.name.clone())))
                .children(p.info.iter().map(|l| div().child(l.clone())))
                .child(
                    div().h_flex().flex_wrap().gap_x_3().children(
                        seen.split(" · ")
                            .filter(|p| !p.is_empty())
                            .map(|p| caption().whitespace_nowrap().child(p.to_string())),
                    ),
                )
                .when(!p.channels.is_empty(), |d| d.child(caption().child(format!("channels {}", p.channels))))
        }
    };
    div().v_flex().gap_2().child(ui::header("PROFILE")).child(body)
}

pub(super) fn match_settings_rows(s: &MatchSettings) -> [(&'static str, String); 5] {
    let slots = match (s.open, s.playing) {
        (Some(0), _) => "Closed to new matches".to_string(),
        (Some(limit), Some(playing)) => {
            format!("{playing} playing / {limit} allowed{}", if playing >= limit { " (full)" } else { "" })
        }
        (Some(limit), None) => format!("Up to {limit} simultaneous matches"),
        (None, _) => "Not reported".to_string(),
    };
    let text = |value: Option<&str>, empty: &str| match value {
        None => "Not reported".to_string(),
        Some("") => empty.to_string(),
        Some(v) => v.to_string(),
    };
    [
        ("Match slots", slots),
        (
            "Rated setting",
            match s.rated {
                Some(true) => "Enabled",
                Some(false) => "Disabled",
                None => "Not reported",
            }
            .to_string(),
        ),
        ("Auto accept", text(s.accept.as_deref(), "Not configured")),
        ("Auto decline", text(s.decline.as_deref(), "Not configured")),
        (
            "Trust",
            match s.trust {
                Some(true) => "Enabled (+)",
                Some(false) => "Disabled (-)",
                None => "Not reported",
            }
            .to_string(),
        ),
    ]
}

pub(super) fn render_match_settings(settings: Option<&Option<MatchSettings>>) -> Div {
    let body = div().v_flex().gap_1().min_w_0().text_sm();
    let body = match settings {
        None => body.text_color(ui::muted()).child(ui::LOADING),
        Some(None) => body.text_color(ui::muted()).child("No Othello match settings."),
        Some(Some(s)) => body
            .children(match_settings_rows(s).into_iter().map(|(label, value)| {
                div().h_flex().items_start().gap_3().min_w_0()
                    .child(div().w(px(112.)).flex_none().text_color(ui::muted()).child(label))
                    .child(div().flex_1().min_w_0().font_family(ui::MONO).child(value))
            }))
            .child(div().pt_1().text_xs().text_color(ui::muted()).child("Decline beats accept. Without an accept rule, requests wait for manual acceptance. A free slot is required."))
            .child(div().text_xs().text_color(ui::muted()).child("Rule keys: size, rated, synchro (paired games), rand (random start).")),
    };
    div().v_flex().gap_2().min_w_0().child(ui::header("MATCH REQUESTS")).child(body)
}

/// One `history` or `stored` row of `login`'s games, with the result on `login`'s side; a click replays it.
pub(super) fn archived_row(section: &str, a: &Archived, login: &str, cx: &mut Context<Viewer>) -> Stateful<Div> {
    let key = a.id.clone();
    let window_key = key.clone();
    let result = a.margin_for(login).map(|m| match m {
        m if m > 0. => (format!("W {m:+}"), ui::accent()),
        m if m < 0. => (format!("L {m:+}"), ui::live()),
        _ => ("D".to_string(), ui::muted()),
    });
    let selector = format!("archive-{section}-{}", a.id);
    row(SharedString::from(format!("{section}{}", a.id)))
        .debug_selector(move || selector.clone())
        .h_flex()
        .gap_3()
        .py_1()
        .bg(ui::surface())
        .cursor_pointer()
        .hover(|s| s.bg(ui::cursor_bg()))
        .font_family(ui::MONO)
        .text_xs()
        .child(div().w(px(40.)).child(a.id.clone()))
        .child(div().text_color(ui::muted()).child(a.date.clone()))
        .child(div().flex_1().min_w_0().truncate().text_sm().child(format!("{} vs {}", a.player1, a.player2)))
        .child(
            div()
                .w(px(56.))
                .text_sm()
                .font_weight(FontWeight::BOLD)
                .when_some(result, |d, (text, color)| d.text_color(color).child(text)),
        )
        .child(div().w(px(56.)).truncate().text_color(ui::muted()).child(a.info.clone()))
        .child(window_button(format!("archive-window-{section}-{}", a.id)).on_click(cx.listener(
            move |this, _: &ClickEvent, _, cx| {
                cx.stop_propagation();
                this.open_archive_window(&window_key, cx);
            },
        )))
        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.look(key.clone(), cx)))
}

pub(super) fn window_button(id: String) -> Button {
    Button::new(SharedString::from(id.clone()))
        .ghost()
        .xsmall()
        .cursor_pointer()
        .icon(IconName::ExternalLink)
        .tooltip("Open in separate window")
        .debug_selector(move || id.clone())
}

pub(super) fn user_row(i: usize, u: &User, cx: &mut Context<Viewer>) -> Stateful<Div> {
    let name = u.name.clone();
    let status = match u.status {
        UserStatus::Open => "open",
        UserStatus::Busy => "busy",
        UserStatus::Ghost => "ghost",
    };
    row(("user", i))
        .v_flex()
        .gap_0p5()
        .py_2()
        .bg(ui::surface())
        .cursor_pointer()
        .hover(|s| s.bg(ui::cursor_bg()))
        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.open_games(name.clone(), false, cx)))
        .child(
            div()
                .h_flex()
                .gap_2()
                .child(div().flex_1().min_w_0().truncate().font_weight(FontWeight::SEMIBOLD).child(u.name.clone()))
                .child(ui::dot(ui::accent()).when(u.status != UserStatus::Open, |d| d.invisible())),
        )
        .child(caption().child(format!(
            "{} · {status}{}",
            u.rating,
            u.playing.as_ref().map_or(String::new(), |m| format!(" · in {m}"))
        )))
}

/// The card the match, user, ranking and archive rows start from; its border shows only when a row sets a color.
pub(super) fn row(id: impl Into<ElementId>) -> Stateful<Div> {
    div().id(id).px_3().rounded(px(ui::RADIUS_ROW)).border_1().border_color(ui::surface()).bg(ui::surface())
}

/// The small muted monospace line under a title.
pub(super) fn caption() -> Div {
    div().font_family(ui::MONO).text_xs().text_color(ui::muted())
}
