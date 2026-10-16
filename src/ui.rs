//! Every color, radius and size the screens use, plus the few building blocks they share.
//! "Graphite", dark or light: quiet monochrome with a green accent for what is live or chosen, and a color per side for evals.

pub use gpui_kit::assets::IconName;
use gpui_kit::component::scroll::ScrollbarMode;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{Icon, StyledExt, Theme, ThemeMode};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;

use std::sync::atomic::{AtomicBool, Ordering};

use crate::othello::Color;

static LIGHT: AtomicBool = AtomicBool::new(false);

fn pick(dark: u32, light: u32) -> Rgba {
    rgb(if LIGHT.load(Ordering::Relaxed) { light } else { dark })
}

/// The theme the user picked; `System` follows the OS. Kept under `theme` in the settings table.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ThemeChoice {
    System,
    Light,
    Dark,
}

impl ThemeChoice {
    pub fn load(db: &rusqlite::Connection) -> ThemeChoice {
        let name = crate::store::setting(db, "theme");
        [ThemeChoice::Light, ThemeChoice::Dark].into_iter().find(|c| c.name() == name).unwrap_or(ThemeChoice::System)
    }

    pub fn save(self, db: &rusqlite::Connection) -> rusqlite::Result<()> {
        crate::store::put_setting(db, "theme", self.name())
    }

    pub fn name(self) -> &'static str {
        match self {
            ThemeChoice::System => "System",
            ThemeChoice::Light => "Light",
            ThemeChoice::Dark => "Dark",
        }
    }

    pub fn icon(self) -> IconName {
        match self {
            ThemeChoice::System => IconName::SunMoon,
            ThemeChoice::Light => IconName::Sun,
            ThemeChoice::Dark => IconName::Moon,
        }
    }

    pub fn next(self) -> ThemeChoice {
        match self {
            ThemeChoice::System => ThemeChoice::Light,
            ThemeChoice::Light => ThemeChoice::Dark,
            ThemeChoice::Dark => ThemeChoice::System,
        }
    }

    pub fn is_light(self, os: WindowAppearance) -> bool {
        match self {
            ThemeChoice::System => matches!(os, WindowAppearance::Light | WindowAppearance::VibrantLight),
            ThemeChoice::Light => true,
            ThemeChoice::Dark => false,
        }
    }
}

/// The window size and panel sizes left at the last close, under `layout` in the settings table as
/// `width height maximized left right log`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Layout {
    pub width: f32,
    pub height: f32,
    pub maximized: bool,
    pub left: f32,
    pub right: f32,
    pub log: f32,
}

impl Default for Layout {
    fn default() -> Self {
        Layout { width: 1240., height: 920., maximized: false, left: LEFT_W, right: RIGHT_W, log: LOG_H }
    }
}

impl Layout {
    /// A missing or unreadable row is the default layout.
    pub fn load(db: &rusqlite::Connection) -> Layout {
        Layout::parse(&crate::store::setting(db, "layout")).unwrap_or_default()
    }

    fn parse(text: &str) -> Option<Layout> {
        let v: Vec<f32> = text.split(' ').map(str::parse).collect::<Result<_, _>>().ok()?;
        let &[width, height, maximized, left, right, log] = v.as_slice() else { return None };
        v.iter().all(|x| x.is_finite() && *x >= 0.).then_some(Layout {
            width,
            height,
            maximized: maximized != 0.,
            left,
            right,
            log,
        })
    }

    pub fn save(self, db: &rusqlite::Connection) -> rusqlite::Result<()> {
        let Layout { width, height, maximized, left, right, log } = self;
        let text = format!("{width} {height} {} {left} {right} {log}", u8::from(maximized));
        crate::store::put_setting(db, "layout", &text)
    }
}

pub fn bg() -> Rgba {
    pick(0x111215, 0xf0f2f5)
}
/// The left and right columns.
pub fn column() -> Rgba {
    pick(0x16171b, 0xe8ebef)
}
pub fn surface() -> Rgba {
    pick(0x1f2126, 0xffffff)
}
pub fn line() -> Rgba {
    pick(0x464a53, 0xd3d6dc)
}
pub fn ink() -> Rgba {
    pick(0xf4f4f5, 0x18181b)
}
/// The move-list cursor.
pub fn bright() -> Rgba {
    pick(0xffffff, 0x000000)
}
pub fn muted() -> Rgba {
    pick(0xa3a6ae, 0x60646c)
}
pub fn cursor_bg() -> Rgba {
    pick(0x3a3e46, 0xe2e5ea)
}
pub fn felt() -> Rgba {
    rgb(0x2f7a5c)
}
/// Board frame and grid lines.
pub fn felt_line() -> Rgba {
    rgb(0x123527)
}
pub fn last_ring() -> Rgba {
    rgb(0xffffff)
}
pub fn legal_dot() -> Rgba {
    rgba(0x00000059)
}
/// The side to move, watched games, set toggles and the primary button.
pub fn accent() -> Rgba {
    pick(0x3ecf8e, 0x13875a)
}
fn accent_hover() -> Rgba {
    pick(0x6ee7b7, 0x0f6e49)
}
/// The LIVE tag of a game, a loss or a fall in the standings, and a failed connect on the login screen.
pub fn live() -> Rgba {
    pick(0xf0616d, 0xd6304a)
}
/// A connection that is being opened or retried, and a tournament game still being played.
pub fn pending() -> Rgba {
    pick(0xf5a524, 0xb86e00)
}
/// The places of the top three in a tournament, first to third.
pub fn medal(place: usize) -> Option<Rgba> {
    match place {
        1 => Some(pick(0xf5c542, 0xa87900)),
        2 => Some(pick(0xc4c8d0, 0x6b7280)),
        3 => Some(pick(0xd08a4e, 0x9a5a22)),
        _ => None,
    }
}
/// Colors for player and move evaluations.
pub fn eval_ink(c: Color) -> Rgba {
    match c {
        Color::Black => pick(0xf5a524, 0xb86e00),
        Color::White => pick(0x5ab4f5, 0x1f6fc2),
    }
}

pub const MONO: &str = "Consolas";
pub const RADIUS: f32 = 10.;
pub const RADIUS_ROW: f32 = 8.;
pub const RADIUS_BOARD: f32 = 8.;
const RADIUS_BUTTON: f32 = 8.;
pub const RADIUS_TAG: f32 = 4.;
pub const TOPBAR_H: f32 = 52.;
pub const LEFT_W: f32 = 320.;
pub const RIGHT_W: f32 = 248.;
pub const LOG_H: f32 = 160.;
/// The rail, center and details minima. The left one fits all rail tabs and Refresh beside them,
/// an unread mark on Chat included. Their sum is the narrowest window; a narrower one squeezes text and buttons together.
pub const SIDE_MINIMA: [f32; 3] = [320., 440., 200.];
pub const COORD: f32 = 18.;
/// Width of the frame around the board; the grid lines between squares are `BOARD_GAP` wide.
pub const BOARD_FRAME: f32 = 3.;
pub const BOARD_GAP: f32 = 2.;
/// Frame plus the outer ring, both sides.
pub const FRAME: f32 = 2. * (BOARD_FRAME + 1.);
pub const LAST_INSET: f32 = 2.;
pub const PANEL_GAP: f32 = 10.;
pub const LOGIN_W: f32 = 380.;
/// The widest a page (help, games, tournament) grows, so its rows stay readable on a wide window.
pub const PAGE_MAX_W: f32 = 1100.;
pub const KIBITZ_H: f32 = 160.;
pub const KIBITZ_MIN: f32 = 140.;
pub const MOVES_MIN: f32 = 80.;
/// The Focus/Synchro row above the board, kept even when it has no tabs.
pub const VIEW_TABS_H: f32 = 44.;
pub const MOVE_NO_W: f32 = 26.;
pub const CLOCK_SIZE: f32 = 20.;
pub const DISC_SM: f32 = 12.;
pub const DISC_RATIO: f32 = 0.78;

/// Switches the palette and maps the component library's theme onto it so inputs, tabs and the primary button match.
pub fn install(light: bool, cx: &mut App) {
    LIGHT.store(light, Ordering::Relaxed);
    Theme::change(if light { ThemeMode::Light } else { ThemeMode::Dark }, None, cx);
    Theme::update(cx, |t| {
        t.background = bg().into();
        t.foreground = ink().into();
        t.border = line().into();
        t.input = line().into();
        t.muted = surface().into();
        t.muted_foreground = muted().into();
        t.primary = accent().into();
        t.primary_hover = accent_hover().into();
        t.primary_active = accent().into();
        t.primary_foreground = bg().into();
        t.button_primary = t.primary;
        t.button_primary_hover = t.primary_hover;
        t.button_primary_active = t.primary_active;
        t.button_primary_foreground = t.primary_foreground;
        t.secondary = surface().into();
        t.secondary_hover = line().into();
        t.secondary_active = line().into();
        t.secondary_foreground = muted().into();
        t.accent = line().into();
        t.accent_foreground = ink().into();
        t.ring = accent().into();
        t.tab_bar = bg().into();
        // The selected segment is drawn in `background`; on the light page a track of that same
        // color left it a faint shadow, so the light track is a shade darker.
        t.tab_bar_segmented = if light { line() } else { bg() }.into();
        t.tab_active = line().into();
        t.tab_active_foreground = accent().into();
        t.tab_foreground = muted().into();
        t.scrollbar_thumb = line().into();
        // A page that runs past the window says so before the wheel is touched.
        t.scrollbar_mode = ScrollbarMode::Always;
    });
}

/// Stands in for content: `Loading...` while a reply is on its way, `No <things>.` once it is empty.
pub const LOADING: &str = "Loading...";

pub fn placeholder(text: impl Into<SharedString>) -> Div {
    div().text_sm().text_color(muted()).child(text.into())
}

pub fn header(label: &str) -> Div {
    div().text_xs().font_weight(FontWeight::SEMIBOLD).text_color(muted()).child(label.to_string())
}

pub fn boxed() -> Div {
    div().v_flex().rounded(px(RADIUS)).border_1().border_color(line()).bg(surface())
}

pub fn disc(color: Color, size: f32) -> Div {
    let (fill, edge) = match color {
        Color::Black => (rgb(0x0c0c0e), rgb(0x5b5e66)),
        Color::White => (rgb(0xf4f4f2), rgb(0x0c0c0e)),
    };
    div().flex_none().size(px(size)).rounded_full().bg(fill).border_1().border_color(edge)
}

pub fn tag(label: impl Into<SharedString>) -> Div {
    div()
        .flex_none()
        .px_1p5()
        .rounded(px(RADIUS_TAG))
        .border_1()
        .border_color(muted())
        .font_family(MONO)
        .text_xs()
        .text_color(ink())
        .child(label.into())
}

pub fn dot(color: Rgba) -> Div {
    div().flex_none().size(px(6.)).rounded_full().bg(color)
}

/// Panel-filled button; only `on` (a toggle that is set) gets a border, in the accent.
pub fn button(id: impl Into<ElementId>, label: impl Into<SharedString>, on: bool, enabled: bool) -> Stateful<Div> {
    div()
        .id(id)
        .flex_none()
        .px(px(9.))
        .py(px(3.))
        .rounded(px(RADIUS_BUTTON))
        .border_1()
        .border_color(if on { accent() } else { surface() })
        .bg(surface())
        .text_xs()
        .text_color(if enabled { ink() } else { muted() })
        .when(enabled, |d| d.cursor_pointer().hover(|s| s.bg(cursor_bg())))
        .child(label.into())
}

/// `button` with an icon in place of its label, which the tooltip says instead.
pub fn icon_button(
    id: impl Into<ElementId>,
    icon: IconName,
    tip: impl Into<SharedString>,
    on: bool,
    enabled: bool,
) -> Stateful<Div> {
    let tip = tip.into();
    button(id, "", on, enabled)
        .px(px(6.))
        .h_flex()
        .child(Icon::new(icon).size_3p5())
        .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
}

/// `v` with its sign, `+4` or `-2.5`, but no sign on a zero, which leans neither way.
pub fn signed(v: f64, precision: Option<usize>) -> String {
    let s = match precision {
        Some(p) => format!("{v:+.p$}"),
        None => format!("{v:+}"),
    };
    if s[1..].chars().all(|c| matches!(c, '0' | '.')) { s[1..].to_string() } else { s }
}

pub fn color_name(c: Color) -> &'static str {
    match c {
        Color::Black => "BLACK",
        Color::White => "WHITE",
    }
}

#[cfg(test)]
mod tests {
    use super::{Layout, ThemeChoice, signed};
    use gpui_kit::WindowAppearance;

    #[test]
    fn choices_cycle_and_system_follows_the_os() {
        let mut c = ThemeChoice::System;
        let mut seen = Vec::new();
        for _ in 0..3 {
            seen.push(c.name());
            c = c.next();
        }
        assert_eq!((seen, c), (vec!["System", "Light", "Dark"], ThemeChoice::System));
        assert!(ThemeChoice::System.is_light(WindowAppearance::VibrantLight));
        assert!(!ThemeChoice::System.is_light(WindowAppearance::Dark));
        assert!(ThemeChoice::Light.is_light(WindowAppearance::Dark));
        assert!(!ThemeChoice::Dark.is_light(WindowAppearance::Light));
    }

    #[test]
    fn zero_takes_no_sign() {
        assert_eq!(
            [signed(4., None), signed(-2.5, None), signed(0., None), signed(-0., None)],
            ["+4", "-2.5", "0", "0"]
        );
        assert_eq!([signed(1.333, Some(2)), signed(-0.004, Some(2)), signed(0., Some(1))], ["+1.33", "0.00", "0.0"]);
    }

    #[test]
    fn the_layout_round_trips_and_a_bad_row_is_the_default() {
        let db = crate::store::memory();
        assert_eq!(Layout::load(&db), Layout::default());
        let l = Layout { width: 1000.5, height: 700., maximized: true, left: 280., right: 300., log: 120. };
        l.save(&db).unwrap();
        assert_eq!(Layout::load(&db), l);
        for bad in ["", "1 2 3", "1 2 0 4 5 NaN", "1 2 0 4 5 -6", "1 2 0 4 5 6 7"] {
            assert_eq!(Layout::parse(bad), None, "{bad:?}");
        }
    }
}
