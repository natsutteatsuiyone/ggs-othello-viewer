use super::*;
use crate::search::{Row, Searches, row_of};
use std::cell::RefCell;
use std::rc::Rc;

#[derive(Default)]
pub(super) struct BoardPane {
    pub views: GameViews,
    pub focus_fit: Fit,
    pub synchro_fit: [Fit; 2],
    pub moves_scroll: ScrollHandle,
    /// The engine logs under the board.
    pub search_scroll: LogScroll,
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
    // The position before the live one, to animate a move as it arrives; a finished game or a
    // stepped-back cursor stays still. Keying each animation on the move number restarts it per move.
    let ply = game.moves().len();
    let live = game.phase == Phase::Live && at.is_none();
    let before = ply.checked_sub(1).filter(|_| live).map(|p| game.board(Some(p)));
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
                        |s, color| {
                            let size = (sq_size * ui::DISC_RATIO).floor();
                            let id = SharedString::from(format!("disc-{}-{ply}-{x}-{y}", game.id));
                            match (last, before.map(|p| p.cell(x, y))) {
                                (Some(sq), Some(_)) if sq == here => s.child(placed_disc(id, color, size)),
                                (Some(sq), Some(Cell::Disc(was))) if was != color => {
                                    let ring = sq.x.abs_diff(x).max(sq.y.abs_diff(y));
                                    s.child(flipping_disc(id, was, color, size, ring))
                                }
                                _ => s.child(ui::disc(color, size)),
                            }
                        },
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

const POP: Duration = Duration::from_millis(220);
const FLIP: Duration = Duration::from_millis(260);
/// Each ring of distance from the placed disc starts its flip this much later.
const RIPPLE: Duration = Duration::from_millis(60);

/// The disc just played, popping in with a small overshoot.
fn placed_disc(id: SharedString, color: Color, size: f32) -> impl IntoElement {
    ui::disc(color, size).with_animation(id, Animation::new(POP), move |d, t| {
        let back = t - 1.;
        let scale = 1. + 2.7 * back.powi(3) + 1.7 * back.powi(2);
        d.size(px(size * scale))
    })
}

/// A disc turning over: it narrows to an edge in its old color, then widens in its new one,
/// waiting its turn by `ring` so the flips ripple out from the move.
fn flipping_disc(id: SharedString, was: Color, now: Color, size: f32, ring: usize) -> impl IntoElement {
    let delay = RIPPLE * ring as u32;
    let total = POP + delay + FLIP;
    let edge = move |t: f32| {
        let local = (t * total.as_secs_f32() - (POP + delay).as_secs_f32()) / FLIP.as_secs_f32();
        ease_in_out(local.clamp(0., 1.))
    };
    div().with_animation(id, Animation::new(total), move |_, t| {
        let t = edge(t);
        let (color, width) = if t < 0.5 { (was, 1. - 2. * t) } else { (now, 2. * t - 1.) };
        ui::disc(color, size).w(px((size * width).max(1.)))
    })
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

/// The logs' scrolling height in a column `column_h` tall, and all the height they take under synchro
/// boards: their padding (24), name row (20) and its gap (8), and the 16 below them. They take a share of
/// the column, and the boards the rest.
pub(super) fn engine_logs_h(column_h: f32) -> (f32, f32) {
    let h = (column_h * 0.3).clamp(140., 420.);
    (h, h + 68.)
}

/// The players as the logs list them, left first: black first, as the player panels show them, unless
/// flipped. The colors swap on a synchro match's second board, but its logs keep the first board's order,
/// so switching boards leaves each player's column where it was.
fn log_players(game: &Game, flipped: bool) -> [&str; 2] {
    let (b, w) = (game.black.name.as_str(), game.white.name.as_str());
    let second = game.is_synchro() && game.id.ends_with(".1");
    if flipped != second { [w, b] } else { [b, w] }
}

/// The engine logs' list, the rows it holds, and what it showed when last drawn. Only the rows in view are
/// drawn, so a long tournament's logs cost no more than a short one's.
pub(super) struct LogScroll {
    pub list: ListState,
    rows: RefCell<Rc<Vec<Row>>>,
    /// The players and `Searches::added` the rows were built for.
    built: RefCell<Option<([String; 2], u64)>>,
    shown: std::cell::Cell<Option<(String, At, bool)>>,
}

impl Default for LogScroll {
    fn default() -> Self {
        Self {
            list: ListState::new(0, ListAlignment::Top, px(200.)),
            rows: Default::default(),
            built: Default::default(),
            shown: Default::default(),
        }
    }
}

impl LogScroll {
    /// The players' rows, built again only when a log or move arrived or the players changed. The list keeps
    /// the rows that did not change, so it keeps their sizes and the scroll.
    fn rows(&self, searches: &Searches, players: [&str; 2]) -> Rc<Vec<Row>> {
        let key = Some((players.map(str::to_string), searches.added()));
        let mut built = self.built.borrow_mut();
        let mut held = self.rows.borrow_mut();
        if *built != key {
            let rows = searches.rows(players);
            if built.as_ref().map(|(p, _)| p) == key.as_ref().map(|(p, _)| p) {
                let kept = held.iter().zip(&rows).take_while(|(a, b)| a == b).count();
                self.list.splice(kept..held.len(), rows.len() - kept);
            } else {
                self.list.reset(rows.len());
            }
            *built = key;
            *held = Rc::new(rows);
        }
        held.clone()
    }

    /// Moves the scroll when another board, position or flip is shown, or the logs appear: to the rule of the
    /// move that led to `at`, or live to the newest rows, which new rows then keep in view until the reader
    /// scrolls up. An ended game goes to its last move, since the players' later games follow it.
    fn sync(&self, rows: &[Row], game: &Game, at: At, flipped: bool) {
        let shown = Some((game.id.clone(), at, flipped));
        if self.shown.replace(shown.clone()) == shown {
            return;
        }
        match at.or((game.phase != Phase::Live).then(|| game.moves().len())) {
            None => self.list.set_follow_mode(FollowMode::Tail),
            Some(k) => {
                self.list.set_follow_mode(FollowMode::Normal);
                if let Some(i) = row_of(rows, &game.id, k) {
                    self.list.scroll_to_reveal_item(i);
                }
            }
        }
    }

    /// The logs are not drawn; they move to their place when they next are.
    pub(super) fn forget(&self) {
        self.shown.set(None);
    }
}

/// The room between the logs' two columns, the center line in its middle.
const LOG_GUTTER: f32 = 32.;
/// Room at the logs' right edge for the scrollbar, which overlays it; the names keep it too, to stay over
/// their columns.
const LOG_SCROLLBAR: f32 = 12.;

/// The two players' engine logs under the board, one column each under their player panels: what each sent
/// to its log channel, as it was sent, in arrival order. A rule across both columns marks each move the
/// server announced, so what the two players sent between the same two moves shares a row. A player's
/// engine logs both boards of a synchro match to one channel, so the rules of both boards are there.
/// Browsing, the rule of the move that led to `at` is marked and scrolled to. The logs keep `height`, so a
/// line arriving never resizes the board.
pub(super) fn engine_logs(
    searches: &Searches,
    scroll: &LogScroll,
    game: &Game,
    at: At,
    flipped: bool,
    width: f32,
    height: f32,
) -> Div {
    let players = log_players(game, flipped);
    let rows = scroll.rows(searches, players);
    scroll.sync(&rows, game, at, flipped);
    let anchor = at.and_then(|k| row_of(&rows, &game.id, k));
    let names = players.map(str::to_string);
    let column = || div().flex_1().min_w_0().v_flex();
    let name = |n: &str| {
        let selector = format!("search-name-{n}");
        column()
            .debug_selector(move || selector.clone())
            .h(px(20.))
            .truncate()
            .font_weight(FontWeight::SEMIBOLD)
            .child(n.to_string())
    };
    let list =
        list(scroll.list.clone(), move |i, _, _| log_row(&rows[i], &names, anchor == Some(i)).into_any_element());
    div()
        .debug_selector(|| "search".into())
        .w(px(width))
        .v_flex()
        .gap_2()
        .px_4()
        .py_3()
        .rounded(px(ui::RADIUS))
        .bg(ui::surface())
        .text_sm()
        .relative()
        // A faint line between the columns, from the names to the bottom, unbroken by the rules.
        .child(
            div().absolute().top_3().bottom_3().left(px((width - LOG_SCROLLBAR) / 2. - 0.5)).w(px(1.)).bg(ui::line()),
        )
        .child(div().h_flex().gap(px(LOG_GUTTER)).pr(px(LOG_SCROLLBAR)).child(name(players[0])).child(name(players[1])))
        // The scrollbar overlays a still frame, as the center column's does.
        .child(
            div()
                .debug_selector(|| "search-body".into())
                .relative()
                .h(px(height))
                .font_family(ui::MONO)
                .text_xs()
                .child(list.size_full().pr(px(LOG_SCROLLBAR)))
                .vertical_scrollbar(&scroll.list),
        )
}

/// One row of the logs: each player's lines side by side, or a move's rule across both columns with its
/// label in the mover's.
fn log_row(row: &Row, players: &[String; 2], anchored: bool) -> Div {
    // A list item is as wide as its content unless told otherwise.
    let row_div = || div().w_full();
    match row {
        Row::Lines(columns) => {
            row_div().py_1().h_flex().items_start().gap(px(LOG_GUTTER)).children(columns.iter().map(|lines| {
                div()
                    .flex_1()
                    .min_w_0()
                    .v_flex()
                    .gap_0p5()
                    .children(lines.iter().map(|l| div().whitespace_normal().child(l.clone())))
            }))
        }
        Row::Move { ply, games } => {
            let label = |side: usize| {
                games[side].as_ref().map(|game| {
                    let selector = format!("search-move-{}-{game}-{ply}", players[side]);
                    div().debug_selector(move || selector.clone()).flex_none().px_2().child(format!("{game} · {ply}"))
                })
            };
            let rule = || div().flex_1().h(px(1.)).bg(if anchored { ui::accent() } else { ui::line() });
            let selector = format!("search-rule-{ply}");
            row_div()
                .debug_selector(move || selector.clone())
                .h(px(20.))
                .my_1()
                .h_flex()
                .text_color(if anchored { ui::accent() } else { ui::muted() })
                .child(rule())
                .children(label(0))
                .child(rule())
                .child(rule())
                .children(label(1))
                .child(rule())
        }
    }
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
                l.focus = Some(game.to_string());
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

/// The Focus and Synchro tabs; `prefix` names their debug selectors, as in `{prefix}-focus-tab`.
pub(super) fn view_tabs<T: 'static>(
    id: &'static str,
    prefix: &'static str,
    mode: ViewMode,
    cx: &Context<T>,
    pick: fn(&mut T, ViewMode, &mut Context<T>),
) -> TabBar {
    let views = [(ViewMode::Focus, "Focus", "focus"), (ViewMode::Synchro, "Synchro", "synchro")];
    TabBar::new(id)
        .segmented()
        .small()
        .selected_index(views.iter().position(|(m, ..)| *m == mode).unwrap_or(0))
        .on_click(cx.listener(move |this, ix: &usize, _, cx| {
            if let Some(&(mode, ..)) = views.get(*ix) {
                pick(this, mode, cx);
            }
        }))
        .children(views.into_iter().map(move |(_, label, name)| {
            Tab::new().cursor_pointer().label(label).debug_selector(move || format!("{prefix}-{name}-tab"))
        }))
}
