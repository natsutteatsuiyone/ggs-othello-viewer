use super::*;

#[derive(Default)]
pub(super) struct BoardPane {
    pub views: GameViews,
    pub focus_fit: Fit,
    pub synchro_fit: [Fit; 2],
    pub moves_scroll: ScrollHandle,
}

/// The Focus column and its board as last laid out. Everything in the column but the board keeps
/// its height, so the board gets whatever room that leaves.
#[derive(Clone, Copy, Default)]
pub(super) struct Fit {
    column: f32,
    board: f32,
}

impl Fit {
    /// Height for the board, coordinates included, when the column has `height` to fill.
    pub(super) fn board_room(self, height: f32) -> f32 {
        // Before the first layout, a guess the next frame corrects.
        let rest = if self.column > 0. { self.column - self.board } else { 300. };
        height - rest
    }

    pub(super) fn column(&mut self, s: Size<Pixels>) -> bool {
        std::mem::replace(&mut self.column, f32::from(s.height)) != f32::from(s.height)
    }

    pub(super) fn board(&mut self, s: Size<Pixels>) -> bool {
        std::mem::replace(&mut self.board, f32::from(s.height)) != f32::from(s.height)
    }
}

/// Stores an element's laid-out size on `entity`, redrawing when `store` reports a change.
pub(super) fn measure<T: 'static>(
    entity: WeakEntity<T>,
    store: impl FnOnce(&mut T, Size<Pixels>) -> bool + 'static,
) -> impl FnOnce(Bounds<Pixels>, &mut Window, &mut App) + 'static {
    move |bounds, window, cx| {
        window.defer(cx, move |_, cx| {
            let _ = entity.update(cx, |this, cx| {
                if store(this, bounds.size) {
                    cx.notify();
                }
            });
        });
    }
}

/// How a finished game ended; `None` while it is live.
fn result_text(game: &Game) -> Option<String> {
    match &game.phase {
        Phase::Over(outcome) => Some(outcome.describe().join("  /  ")),
        Phase::Live => None,
    }
}

/// The game's state, LIVE in red; the move counter under the board gives the position shown.
pub(super) fn state_tag(game: &Game) -> Div {
    let state = match &game.phase {
        Phase::Live => "LIVE",
        Phase::Over(_) if game.source == GameSource::Archive => "ARCHIVE",
        Phase::Over(_) => "ENDED",
    };
    ui::tag(state).when(game.phase == Phase::Live, |t| t.border_color(ui::live()).text_color(ui::live()))
}

/// `on_board` gets the board's laid-out bounds.
pub(super) fn focus_position(
    game: &Game,
    view: crate::game::GameView,
    now: Instant,
    cell: f32,
    width: f32,
    on_board: impl FnOnce(Bounds<Pixels>, &mut Window, &mut App) + 'static,
) -> Div {
    let position = view.cursor.unwrap_or(game.moves().len());
    div()
        .v_flex()
        .items_center()
        .gap_3()
        .debug_selector(move || format!("focus-position-{position}"))
        .child(player_panel(game, view.cursor, view.flipped, now, width))
        .when_some(result_text(game), |d, result| {
            d.child(div().w(px(width)).font_family(ui::MONO).text_sm().child(format!("result  {result}")))
        })
        .child(
            render_board(game, view.cursor, BoardOpts { cell, hints: view.hints, flipped: view.flipped, coords: true })
                .debug_selector(|| "focus-board".into())
                .relative()
                .on_prepaint(on_board),
        )
}

/// Cell size that fits an `n`-wide board plus its coordinate margins into `room`.
pub(super) fn cell_size(room: Size<f32>, n: usize) -> f32 {
    ((room.width.min(room.height) - ui::COORD - ui::FRAME) / n as f32).floor().clamp(14., 60.)
}

#[derive(Clone, Copy)]
pub(super) struct BoardOpts {
    pub(super) cell: f32,
    pub(super) hints: bool,
    pub(super) flipped: bool,
    pub(super) coords: bool,
}

/// Squares on a felt-line background; the gaps between them draw the grid lines.
pub(super) fn render_board(game: &Game, at: At, BoardOpts { cell, hints, flipped, coords }: BoardOpts) -> Div {
    let b = game.board(at);
    let last = game.last_square(at);
    let hints = hints && game.shows_turn(at);
    let sq_size = cell - ui::BOARD_GAP;
    let label = |w: f32, h: f32, s: String| {
        div()
            .w(px(w))
            .h(px(h))
            .flex()
            .items_center()
            .justify_center()
            .font_family(ui::MONO)
            .text_xs()
            .text_color(ui::muted())
            .child(s)
    };
    let axis = |n: usize| -> Vec<usize> { if flipped { (0..n).rev().collect() } else { (0..n).collect() } };
    let (xs, ys) = (axis(b.size()), axis(b.size()));
    let files = || {
        div()
            .flex()
            .pl(px(ui::COORD + ui::FRAME / 2.))
            .children(xs.iter().map(|&x| label(cell, ui::COORD, ((b'a' + x as u8) as char).to_string())))
    };
    let rows = ys.iter().map(|&y| {
        div().flex().gap(px(ui::BOARD_GAP)).children(xs.iter().map(move |&x| {
            let here = Square { x, y };
            let hint_selector = format!("legal-hint-{}-{x}-{y}", game.id);
            let sq = div().relative().size(px(sq_size)).flex().items_center().justify_center();
            match b.cell(x, y) {
                Cell::Off => sq.bg(ui::bg()),
                c => sq
                    .bg(ui::felt())
                    .when_some(
                        match c {
                            Cell::Disc(color) => Some(color),
                            _ => None,
                        },
                        |s, color| s.child(ui::disc(color, (sq_size * ui::DISC_RATIO).floor())),
                    )
                    .when(last == Some(here), |s| {
                        s.child(
                            div()
                                .absolute()
                                .inset(px(ui::LAST_INSET))
                                .rounded(px(ui::RADIUS_TAG))
                                .border_2()
                                .border_color(ui::last_ring()),
                        )
                    })
                    .when(hints && b.is_legal(here), |s| {
                        s.child(
                            div()
                                .debug_selector(move || hint_selector.clone())
                                .size(px((cell * 0.2).max(4.)))
                                .rounded_full()
                                .bg(ui::legal_dot()),
                        )
                    }),
            }
        }))
    });
    let grid = div().flex_none().rounded(px(ui::RADIUS_BOARD + 1.)).border_1().border_color(ui::line()).child(
        div()
            .v_flex()
            .gap(px(ui::BOARD_GAP))
            .p(px(ui::BOARD_FRAME))
            .rounded(px(ui::RADIUS_BOARD))
            .overflow_hidden()
            .bg(ui::felt_line())
            .children(rows),
    );
    let ranks = || {
        div().v_flex().pt(px(ui::FRAME / 2.)).children(ys.iter().map(|&y| label(ui::COORD, cell, (y + 1).to_string())))
    };
    if !coords {
        return grid;
    }
    div().flex_none().v_flex().child(files()).child(div().flex().items_start().child(ranks()).child(grid))
}

/// Two boxes side by side; the side to move is outlined in the accent color, and a running clock
/// is in ink, an idle one muted.
pub(super) fn player_panel(game: &Game, at: At, flipped: bool, now: Instant, width: f32) -> Div {
    let board = game.board(at);
    let clock_size = ui::CLOCK_SIZE.min((width / 14.).floor().max(18.));
    let cell = |color: Color| {
        let p = match color {
            Color::Black => &game.black,
            Color::White => &game.white,
        };
        let to_move = game.shows_turn(at) && board.to_move() == color;
        let running = game.is_running(color, at);
        let side = if color == Color::Black { "black" } else { "white" };
        let clock_selector = format!("player-clock-{}-{side}", game.id);
        let cell_selector = format!("player-cell-{}-{side}", game.id);
        div()
            .debug_selector(move || cell_selector.clone())
            .flex_1()
            .min_w_0()
            .v_flex()
            .px_3()
            .py_2()
            .rounded(px(ui::RADIUS))
            // Only the side to move is outlined.
            .border_1()
            .border_color(if to_move { ui::accent() } else { ui::surface() })
            .bg(ui::surface())
            .child(
                div()
                    .h_flex()
                    .gap_2()
                    .child(ui::disc(color, ui::DISC_SM))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_base()
                            .font_weight(FontWeight::BOLD)
                            .child(p.name.clone()),
                    )
                    .child(
                        div()
                            .flex_none()
                            .font_family(ui::MONO)
                            .text_xs()
                            .text_color(ui::muted())
                            .child(format!("{:.0}", p.rating)),
                    ),
            )
            .child(
                // The elapsed time shares the evaluation's row: beside the clock it overflows a narrow card.
                div()
                    .h(px(24.))
                    .flex_shrink_0()
                    .h_flex()
                    .justify_between()
                    .gap_1()
                    .font_family(ui::MONO)
                    .child(
                        div()
                            .id(SharedString::from(format!("evaluation-{}-{}", game.id, ui::color_name(color))))
                            .text_base()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(if game.last_eval(color, at).is_some() {
                                ui::eval_ink(color)
                            } else {
                                ui::muted()
                            })
                            .child(game.last_eval(color, at).map(eval_text).unwrap_or_else(|| "—".into()))
                            .tooltip(|window, cx| {
                                gpui_kit::component::tooltip::Tooltip::new("Latest evaluation reported by this player")
                                    .build(window, cx)
                            }),
                    )
                    .when_some(game.move_elapsed_text(color, at, now), |row, elapsed| {
                        let selector = format!("player-elapsed-{}", game.id);
                        row.child(
                            div()
                                .id(SharedString::from(selector.clone()))
                                .debug_selector(move || selector.clone())
                                .h_flex()
                                .gap_1()
                                .font_family(ui::MONO)
                                .text_xs()
                                .line_height(px(18.))
                                .whitespace_nowrap()
                                .text_color(ui::muted())
                                .child(Icon::new(ui::IconName::Hourglass).size_3())
                                .child(elapsed)
                                .tooltip(|window, cx| {
                                    gpui_kit::component::tooltip::Tooltip::new(
                                        "Elapsed time since the last move update",
                                    )
                                    .build(window, cx)
                                }),
                        )
                    }),
            )
            .child(
                div()
                    .debug_selector(move || clock_selector.clone())
                    .flex_none()
                    .w(px(clock_size * 5.4))
                    .h(px(clock_size * 1.2))
                    .font_family(ui::MONO)
                    .text_size(px(clock_size))
                    .line_height(px(clock_size * 1.2))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(if running { ui::ink() } else { ui::muted() })
                    .child(game.clock_text(color, at, now)),
            )
    };
    let (left, right) = if flipped { (Color::White, Color::Black) } else { (Color::Black, Color::White) };
    let panel_selector = format!("player-panel-{}", game.id);
    div()
        .debug_selector(move || panel_selector.clone())
        .w(px(width))
        .flex()
        .gap(px(ui::PANEL_GAP))
        .child(cell(left))
        .child(cell(right))
}

pub(super) fn under_board<T: 'static>(
    game: &Game,
    at: At,
    width: f32,
    cx: &Context<T>,
    navigate: fn(&mut T, Nav, &mut Context<T>),
) -> Div {
    let n = game.moves().len();
    let k = at.unwrap_or(n);
    let counter = format!("move {k} / {n}");
    let (nb, nw) = game.board(at).count();
    let nav = |id: &'static str, icon: ui::IconName, tip: &'static str, target: Nav, enabled: bool| {
        ui::icon_button(id, icon, tip, false, enabled)
            .when(enabled, |b| b.on_click(cx.listener(move |this, _: &ClickEvent, _, cx| navigate(this, target, cx))))
    };
    div()
        .w(px(width))
        .debug_selector(|| "board-controls".into())
        .h_flex()
        .justify_between()
        .gap_2()
        .child(disc_count(nb, nw).text_lg())
        .child(
            caption()
                .debug_selector({
                    let counter = counter.clone();
                    move || counter.clone()
                })
                .child(counter),
        )
        .child(
            div()
                .h_flex()
                .gap_1p5()
                .child(nav("first", ui::IconName::ChevronFirst, "First move", Nav::First, k > 0))
                .child(nav("prev", ui::IconName::ChevronLeft, "Previous move", Nav::Prev, k > 0))
                .child(nav("next", ui::IconName::ChevronRight, "Next move", Nav::Next, at.is_some()))
                .child(
                    ui::button("live", if game.phase == Phase::Live { "LIVE" } else { "END" }, at.is_none(), true)
                        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| navigate(this, Nav::Live, cx))),
                ),
        )
}

/// Disc counts with a disc of each color, black first.
pub(super) fn disc_count(nb: usize, nw: usize) -> Div {
    div()
        .h_flex()
        .gap_1p5()
        .font_family(ui::MONO)
        .child(ui::disc(Color::Black, ui::DISC_SM))
        .child(nb.to_string())
        .child(div().w_1())
        .child(ui::disc(Color::White, ui::DISC_SM))
        .child(nw.to_string())
}

pub(super) fn render_synchro(lobby: &Lobby, room: Size<f32>, pane: &BoardPane, cx: &Context<Viewer>) -> Div {
    let Some(match_id) = lobby.focus.as_deref().map(match_of) else {
        return empty_state("Pick a synchro match first.");
    };
    render_match_boards(
        match_id,
        &lobby.synchro_halves(),
        room,
        pane,
        cx,
        |v| &mut v.board,
        |v, game, cx| {
            v.with_lobby(cx, |l| {
                l.focus_on(game.to_string());
                l.show_view(ViewMode::Focus)
            });
        },
    )
}

/// The synchro total's row above the cards.
const TOTAL_H: f32 = 24.;

/// `room` is the whole area, padding included; `pane` holds each card as last laid out, and `slot`
/// finds it again to store new sizes.
/// Clicking a card hands its game id to `open`.
pub(super) fn render_match_boards<T: 'static>(
    match_id: &str,
    games: &[Option<&Game>],
    room: Size<f32>,
    pane: &BoardPane,
    cx: &Context<T>,
    slot: fn(&mut T) -> &mut BoardPane,
    open: fn(&mut T, &str, &mut Context<T>),
) -> Div {
    let now = Instant::now();
    let total = match games {
        [Some(a), Some(b)] => match (&a.phase, &b.phase) {
            (Phase::Over(proto::Outcome::Score(a)), Phase::Over(proto::Outcome::Score(b))) => {
                Some(proto::MatchResult::synchro(a, b))
            }
            _ => None,
        },
        _ => None,
    };
    // The total's row and the gap under it come out of the boards' height.
    let room = size(room.width, room.height - if total.is_some() { TOTAL_H + 12. } else { 0. });
    let side_by_side = games.len() == 1 || room.width >= 600.;
    let card_w = if side_by_side {
        ((room.width - 12. * (games.len() - 1) as f32) / games.len() as f32).floor()
    } else {
        room.width.floor()
    };
    let inner_w = (card_w - 24.).max(0.);
    let cards = games.iter().copied().enumerate().map(|(i, game)| {
        let Some(game) = game else {
            let selector = format!("synchro-waiting-{i}");
            return div()
                .id(("synchro-waiting", i))
                .debug_selector(move || selector.clone())
                .w(px(card_w))
                .h(px((card_w * 1.2).clamp(300., 440.)))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(ui::RADIUS))
                .border_1()
                .border_color(ui::line())
                .bg(ui::surface())
                .text_color(ui::muted())
                .child(format!(
                    "Loading {}...",
                    if games.len() == 1 { match_id.to_string() } else { format!("{match_id}.{i}") }
                ));
        };
        let at = None;
        let board = game.board(at);
        let (nb, nw) = board.count();
        let n = game.moves().len();
        // The cards share the padded row; a stacked card is as tall as the scroll needs.
        let board_h = if side_by_side { pane.synchro_fit[i].board_room(room.height - 32.).max(160.) } else { inner_w };
        let cell = cell_size(size(inner_w, board_h), board.size());
        let turn = if game.shows_turn(at) {
            format!("{} to move", ui::color_name(board.to_move()))
        } else {
            "final position".to_string()
        };
        let board_selector = format!("synchro-board-{}", game.id);
        let card_selector = format!("synchro-card-{}", game.id);
        let id = game.id.clone();
        div()
            .id(("synchro-card", i))
            .debug_selector(move || card_selector.clone())
            .relative()
            .cursor_pointer()
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| open(this, &id, cx)))
            .on_prepaint(measure(cx.weak_entity(), move |this: &mut T, s| slot(this).synchro_fit[i].column(s)))
            .w(px(card_w))
            .v_flex()
            .gap_2()
            .p_3()
            .rounded(px(ui::RADIUS))
            .border_1()
            .border_color(ui::line())
            .bg(ui::surface())
            .child(
                div()
                    .h_flex()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .font_weight(FontWeight::BOLD)
                            .child(format!("{} · {}", game.id, game.game_type)),
                    )
                    .child(state_tag(game)),
            )
            .child(player_panel(game, at, false, now, inner_w))
            .when_some(result_text(game), |card, result| {
                card.child(div().w(px(inner_w)).font_family(ui::MONO).text_xs().child(format!("result  {result}")))
            })
            .child(
                div().w(px(inner_w)).flex().justify_center().child(
                    render_board(
                        game,
                        at,
                        BoardOpts { cell, hints: pane.views.get(game).hints, flipped: false, coords: true },
                    )
                    .id(("synchro-board", i))
                    .debug_selector(move || board_selector.clone())
                    .relative()
                    .on_prepaint(measure(cx.weak_entity(), move |this: &mut T, s| slot(this).synchro_fit[i].board(s))),
                ),
            )
            .child(
                div()
                    .w(px(inner_w))
                    .h_flex()
                    .gap_2()
                    .font_family(ui::MONO)
                    .text_xs()
                    .child(disc_count(nb, nw).text_base())
                    .child(caption().child(format!("move {n} / {n}")))
                    .child(div().flex_1())
                    .child(caption().child(turn)),
            )
    });
    div()
        .flex()
        .flex_wrap()
        .items_start()
        .gap_3()
        .p_4()
        .when_some(total, |d, total| {
            d.child(
                div()
                    .w_full()
                    .h(px(TOTAL_H))
                    .flex()
                    .items_center()
                    .font_family(ui::MONO)
                    .text_sm()
                    .child(format!("match  {}  (mean of both boards)", total.describe().join("  /  "))),
            )
        })
        .children(cards)
}

pub(super) fn eval_text(e: f64) -> String {
    if e.abs() >= 1e4 { format!("{e:+.1e}") } else { ui::signed(e, Some(1)) }
}

/// 1 when white moved first, so the record's first row has an empty black cell.
pub(super) fn record_offset(game: &Game) -> usize {
    usize::from(game.moves().first().is_some_and(|m| m.color == Color::White))
}

/// Child index in the record of the row holding the move that led to `at`.
pub(super) fn record_row(game: &Game, at: At) -> usize {
    match at.unwrap_or(game.moves().len()) {
        0 => 0,
        k => (k - 1 + record_offset(game)) / 2,
    }
}

/// `shown` is the position on the board and the move that led to it is the cursor.
pub(super) fn render_moves<T: 'static>(
    game: &Game,
    shown: usize,
    scroll: &ScrollHandle,
    cx: &Context<T>,
    navigate: fn(&mut T, Nav, &mut Context<T>),
) -> Stateful<Div> {
    let offset = record_offset(game);
    let rows = (game.moves().len() + offset).div_ceil(2);
    let cell = |i: Option<usize>| {
        let base = div().flex_1().min_w_0();
        let Some((i, m)) = i.and_then(|i| Some((i, game.moves().get(i)?))) else { return base.into_any_element() };
        let current = i + 1 == shown;
        base.child(
            div()
                .id(("mv", i))
                .h_flex()
                .gap_1()
                .px_1()
                .rounded(px(ui::RADIUS_TAG))
                .cursor_pointer()
                .hover(|s| s.bg(ui::surface()))
                .when(current, |s| s.bg(ui::cursor_bg()))
                .child(div().text_color(if current { ui::bright() } else { ui::ink() }).child(m.token.mv.to_string()))
                .child(div().text_color(ui::eval_ink(m.color)).child(m.token.eval.map(eval_text).unwrap_or_default()))
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| navigate(this, Nav::To(i + 1), cx))),
        )
        .into_any_element()
    };
    let rows = (0..rows).map(|r| {
        let b = (2 * r).checked_sub(offset);
        let w = 2 * r + 1 - offset;
        div()
            .h_flex()
            .py_0p5()
            .child(div().w(px(ui::MOVE_NO_W)).text_color(ui::muted()).child((r + 1).to_string()))
            .child(cell(b))
            .child(cell(Some(w)))
    });
    div()
        .id(SharedString::from(format!("moves{}", game.id)))
        .track_scroll(scroll)
        .flex_1()
        .min_h_0()
        .overflow_y_scroll()
        .font_family(ui::MONO)
        .text_xs()
        .children(rows)
}
