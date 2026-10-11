use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use crate::clock::{Clock, ClockSetting, format_secs, hms};
use crate::ggf::{GameRecord, Player};
use crate::othello::{Board, Color, Move, MoveToken, Square};
use crate::proto::{MatchResult, Outcome};

#[derive(Clone, PartialEq, Debug)]
pub enum Phase {
    Live,
    Over(Outcome),
}

#[derive(Clone, PartialEq, Debug)]
pub struct PlayedMove {
    pub color: Color,
    pub token: MoveToken,
}

#[derive(Clone, Debug)]
struct Position {
    board: Board,
    clocks: [Clock; 2],
}

/// Where a game came from: a live server game, or a finished one loaded from the archive with `look`,
/// which is never watched and never changes.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum GameSource {
    Live,
    Archive,
}

/// The game id an archived match is shown under, kept apart from live match ids: the archive counter
/// `.73` is a different number space from match `.73`. A synchro pair adds `.0` and `.1`.
pub(crate) fn archive_id(key: &str) -> String {
    format!("archive {key}")
}

/// A position index into a game: `None` follows the live position, `Some(k)` is after `k` moves.
pub type At = Option<usize>;

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct GameView {
    pub cursor: At,
    pub flipped: bool,
    pub hints: bool,
}

impl Default for GameView {
    fn default() -> Self {
        Self { cursor: None, flipped: false, hints: true }
    }
}

/// Identifies a received game, independently of the server's recyclable match number.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct GameKey(u64);

static NEXT_GAME_KEY: AtomicU64 = AtomicU64::new(1);

/// Browsing state owned by one window. A rejoin keeps the key; a different match gets a new one.
#[derive(Default)]
pub struct GameViews(BTreeMap<GameKey, GameView>);

impl GameViews {
    pub fn get(&self, game: &Game) -> GameView {
        let mut view = self.0.get(&game.key).copied().unwrap_or_default();
        if view.cursor.is_some_and(|k| k >= game.moves.len()) {
            view.cursor = None;
        }
        view
    }

    pub fn get_mut(&mut self, game: &Game) -> &mut GameView {
        let view = self.get(game);
        let stored = self.0.entry(game.key).or_default();
        *stored = view;
        stored
    }

    pub fn clamp(&mut self, game: &Game) {
        if let Some(view) = self.0.get_mut(&game.key)
            && view.cursor.is_some_and(|k| k >= game.moves.len())
        {
            view.cursor = None;
        }
    }

    pub fn navigate(&mut self, game: &Game, nav: Nav) {
        let view = self.get_mut(game);
        view.cursor = step(view.cursor, game.moves.len(), nav);
    }
}

#[derive(Clone, Debug)]
pub struct Game {
    pub key: GameKey,
    pub id: String,
    pub black: Player,
    pub white: Player,
    pub game_type: String,
    pub source: GameSource,
    pub(crate) date: Option<String>,
    record_result: Option<f64>,
    result_reason: Option<String>,
    /// A scored synchro join can omit one move until the other board catches up.
    pending_final_update: bool,
    clocks: [ClockSetting; 2],
    pub komi: Option<f64>,
    komi_bids: [Option<String>; 2],
    moves: Vec<PlayedMove>,
    /// `positions[k]` is the state after `k` moves; kept whole so browsing is a lookup, not a replay.
    positions: Vec<Position>,
    last_move_at: Instant,
    /// The server sends no time left on join, so until the next move the running clock also counts the
    /// time already spent on the current move and reads high.
    clock_estimated: bool,
    pub phase: Phase,
    /// The whole match ended, rather than just this board of a synchro pair.
    pub match_ended: bool,
    pub observers: BTreeSet<String>,
    pub text: Vec<String>,
}

impl Game {
    pub(crate) fn is_synchro_type(game_type: &str) -> bool {
        game_type.starts_with('s') || game_type.starts_with('S')
    }

    pub(crate) fn is_synchro(&self) -> bool {
        Self::is_synchro_type(&self.game_type)
    }

    /// DT is the send time; match notifications and compatible replays identify live rejoins.
    pub(crate) fn same_match(&self, other: &Game) -> bool {
        // shortcut: identical replays can hide ID reuse during disconnect; use a server generation token if one becomes available.
        let same_players = self.black.name == other.black.name && self.white.name == other.white.name;
        let assigned_colors = self.komi_pending()
            && other.komi.is_some()
            && self.black.name == other.white.name
            && self.white.name == other.black.name;
        !self.match_ended
            && self.source == other.source
            && self.game_type == other.game_type
            && self.positions[0].board == other.positions[0].board
            && (same_players || assigned_colors)
            && self.moves.iter().zip(&other.moves).all(|(a, b)| a == b)
    }

    pub(crate) fn matches_record(&self, other: &Game) -> bool {
        self.source == other.source
            && self.date == other.date
            && self.black.name == other.black.name
            && self.white.name == other.white.name
            && self.game_type == other.game_type
            && self.positions[0].board == other.positions[0].board
            && (self.date.is_some() || self.moves.iter().zip(&other.moves).all(|(a, b)| a.token.mv == b.token.mv))
    }

    pub(crate) fn from_record(id: String, rec: GameRecord, now: Instant) -> Result<Game, String> {
        let mut clocks = rec.clocks.map(Clock::new);
        if rec.komi.is_some() {
            // The starting side's bid is recorded as its first move. Only the other
            // bid needs charging here; a zero-time bid still earns its increment.
            let bidder = rec.start.to_move().other() as usize;
            clocks[bidder].charge(rec.komi_time[bidder])?;
        }
        let mut game = Game {
            key: GameKey(NEXT_GAME_KEY.fetch_add(1, Ordering::Relaxed)),
            id,
            black: rec.black,
            white: rec.white,
            game_type: rec.game_type,
            source: GameSource::Live,
            date: rec.date,
            record_result: rec.result,
            result_reason: rec.result_reason,
            pending_final_update: false,
            clocks: rec.clocks,
            komi: rec.komi,
            komi_bids: rec.komi_bids,
            moves: Vec::new(),
            positions: vec![Position { board: rec.start, clocks }],
            last_move_at: now,
            clock_estimated: false,
            phase: Phase::Live,
            match_ended: false,
            observers: BTreeSet::new(),
            text: Vec::new(),
        };
        for (i, token) in rec.moves.into_iter().enumerate() {
            game.play(token, now).map_err(|e| format!("GGF move {}: {e}", i + 1))?;
        }
        if let Some(score) = game.record_result {
            game.phase = Phase::Over(Outcome::Score(MatchResult {
                player1: game.black.name.clone(),
                player2: game.white.name.clone(),
                score,
            }));
            game.pending_final_update = game.is_synchro() && !game.board(None).is_over();
        }
        game.clock_estimated = game.phase == Phase::Live;
        Ok(game)
    }

    /// A finished record from the archive, over from the start with the result its `RE` gives.
    pub fn archived(id: String, rec: GameRecord, now: Instant) -> Result<Game, String> {
        let mut game = Game::from_record(id, rec, now)?;
        game.source = GameSource::Archive;
        // `from_record` has already ended a record whose `RE` gives a score.
        game.end_match(Outcome::finished("unfinished".into()));
        game.clock_estimated = false;
        Ok(game)
    }

    /// Live, or a scored synchro board still owed its final move.
    pub fn takes_moves(&self) -> bool {
        self.phase == Phase::Live || self.pending_final_update
    }

    /// This board's end notice: no more moves, even one a synchro join left owing.
    pub fn end_board(&mut self, outcome: Outcome) {
        self.phase = Phase::Over(outcome);
        self.pending_final_update = false;
    }

    /// The whole match is over; a board still live takes `outcome`, a finished one keeps its own.
    pub fn end_match(&mut self, outcome: Outcome) {
        self.match_ended = true;
        self.pending_final_update = false;
        if self.phase == Phase::Live {
            self.phase = Phase::Over(outcome);
        }
    }

    pub fn play(&mut self, token: MoveToken, now: Instant) -> Result<(), String> {
        if self.match_ended || !self.takes_moves() {
            return Err("game is over".into());
        }
        let mut next = self.positions[self.moves.len()].clone();
        let color = next.board.to_move();
        next.board.apply(token.mv)?;
        next.clocks[color as usize].charge(token.time)?;
        self.positions.push(next);
        self.moves.push(PlayedMove { color, token });
        self.pending_final_update = false;
        self.last_move_at = now;
        self.clock_estimated = false;
        Ok(())
    }

    pub fn moves(&self) -> &[PlayedMove] {
        &self.moves
    }

    fn komi_pending(&self) -> bool {
        self.game_type.contains(['k', 'K']) && self.komi.is_none() && self.moves.is_empty()
    }

    fn index(&self, at: At) -> usize {
        at.unwrap_or(self.moves.len()).min(self.moves.len())
    }

    pub fn board(&self, at: At) -> &Board {
        &self.positions[self.index(at)].board
    }

    /// `last_eval`, but only for a player that has written an evaluation in this game: one that never
    /// does is not claiming 0.
    pub fn stated_eval(&self, color: Color, at: At) -> Option<f64> {
        let mut moves = self.moves[..self.index(at)].iter().filter(|m| m.color == color);
        if moves.any(|m| m.token.stated) { self.last_eval(color, at) } else { None }
    }

    /// The last eval `color` sent up to `at`, from its own side as GGS gives it.
    pub fn last_eval(&self, color: Color, at: At) -> Option<f64> {
        self.moves[..self.index(at)].iter().rev().filter(|m| m.color == color).find_map(|m| m.token.eval)
    }

    /// Whether the side to move means anything at `at`: anywhere in history, or live while the game runs.
    pub fn shows_turn(&self, at: At) -> bool {
        at.is_some() || self.phase == Phase::Live
    }

    /// Only the live position has running clocks.
    pub fn is_running(&self, color: Color, at: At) -> bool {
        at.is_none() && self.phase == Phase::Live && (self.board(None).to_move() == color || self.komi_pending())
    }

    fn clock_secs(&self, color: Color, at: At, now: Instant) -> i64 {
        let running = (!self.is_synchro() && self.is_running(color, at))
            .then(|| now.saturating_duration_since(self.last_move_at));
        self.positions[self.index(at)].clocks[color as usize].display_secs(running)
    }

    /// The main clock readout. Synchro stays at the last confirmed balance; an ordinary live clock
    /// ticks locally and is marked `~` until a move confirms its starting balance after joining.
    pub fn clock_text(&self, color: Color, at: At, now: Instant) -> String {
        let text = format_secs(self.clock_secs(color, at, now));
        if !self.is_synchro() && self.clock_estimated && self.is_running(color, at) { format!("~{text}") } else { text }
    }

    /// Time since the last confirmed position for the active side of a live synchro game.
    pub fn move_elapsed_text(&self, color: Color, at: At, now: Instant) -> Option<String> {
        if !self.is_synchro() || self.clock_estimated || !self.is_running(color, at) {
            return None;
        }
        Some(hms(now.saturating_duration_since(self.last_move_at).as_secs()))
    }

    /// The game as a GGF record that `GameRecord::parse` reads back to what this game was built from.
    pub fn to_ggf(&self) -> String {
        let mut s = format!(
            "(;GM[Othello]PC[GGS/os]PB[{}]PW[{}]RB[{}]RW[{}]",
            self.black.name, self.white.name, self.black.rating, self.white.rating
        );
        let [tb, tw] = self.clocks;
        if let Some(date) = &self.date {
            let _ = write!(s, "DT[{date}]");
        }
        // Writing to a String cannot fail.
        if tb == tw {
            let _ = write!(s, "TI[{tb}]");
        } else {
            let _ = write!(s, "TB[{tb}]TW[{tw}]");
        }
        let _ = write!(s, "TY[{}]", self.game_type);
        for (tag, bid) in ["KB", "KW"].iter().zip(&self.komi_bids) {
            if let Some(b) = bid {
                let _ = write!(s, "{tag}[{b}]");
            }
        }
        if let Some(k) = self.komi {
            let _ = write!(s, "KM[{k}]");
        }
        let result = self.black_result().or(self.record_result);
        let reason = self.result_reason.as_deref().filter(|_| result == self.record_result);
        s += "RE[";
        match result {
            Some(r) => {
                let _ = write!(s, "{r:+.3}");
            }
            None => s += "?",
        }
        if let Some(reason) = reason {
            let _ = write!(s, ":{reason}");
        }
        s += "]";
        let _ = write!(s, "BO[{}]", self.positions[0].board.to_bo());
        for m in &self.moves {
            let _ = write!(s, "{}[{}]", if m.color == Color::Black { "B" } else { "W" }, m.token);
        }
        s + ";)"
    }

    /// Black's margin, the sign GGF's `RE` uses; a match result is given from player 1's side.
    fn black_result(&self) -> Option<f64> {
        match &self.phase {
            Phase::Over(Outcome::Score(r)) => Some(if r.player1 == self.black.name { r.score } else { -r.score }),
            _ => None,
        }
    }

    pub fn last_square(&self, at: At) -> Option<Square> {
        match self.moves.get(self.index(at).checked_sub(1)?)?.token.mv {
            Move::Square(sq) => Some(sq),
            Move::Pass => None,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Nav {
    First,
    Prev,
    Next,
    Live,
    To(usize),
}

/// Cursor after `nav` in a game of `n` moves; reaching the last position resumes following it live.
pub(crate) fn step(cursor: At, n: usize, nav: Nav) -> At {
    let cur = cursor.unwrap_or(n);
    let target = match nav {
        Nav::First => 0,
        Nav::Prev => cur.saturating_sub(1),
        Nav::Next => cur.saturating_add(1),
        Nav::Live => n,
        Nav::To(k) => k,
    };
    (target < n).then_some(target)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::ggf::tests::SPEC_JOIN;
    use crate::lobby::tests::joined;
    use crate::othello::Cell;
    use crate::proto::OsMsg;
    use std::time::Duration;

    pub(crate) fn tok(s: &str) -> MoveToken {
        MoveToken::parse(s).unwrap()
    }

    #[test]
    fn synchro_clock_keeps_increment_and_extension_on_confirmed_moves_only() {
        let now = Instant::now();
        let record = GameRecord::parse(
            "(;GM[Othello]PB[alice]PW[bob]TI[0:10/0:05/1:00]TY[S8]RE[?]\
             BO[8 -------- -------- -------- ---O*--- ---*O--- -------- -------- -------- *];)",
        )
        .unwrap();
        let mut game = Game::from_record(".21.0".into(), record, now).unwrap();

        game.play(tok("d3//12"), now).unwrap();
        assert_eq!(
            game.clock_text(Color::Black, None, now),
            "+0:01:03",
            "the confirmed charge applies extension then increment"
        );
        assert_eq!(
            game.clock_text(Color::White, None, now + Duration::from_secs(12)),
            "+0:00:10",
            "waiting for the paired update must not speculate that the active clock entered its extension",
        );
    }

    #[test]
    #[ignore]
    fn ggf_file_replays() {
        let text = std::fs::read_to_string(std::env::var("GGF_FILE").expect("set GGF_FILE")).unwrap();
        let rec = GameRecord::parse(&text).unwrap();
        let g = Game::from_record("file".into(), rec.clone(), Instant::now()).unwrap();
        println!(
            "{} vs {}, type {}, {} moves, RE {:?}, final {:?}",
            g.black.name,
            g.white.name,
            g.game_type,
            g.moves.len(),
            rec.result,
            g.board(None).count()
        );
    }

    #[test]
    fn browsing_steps_between_the_first_position_and_live() {
        use Nav::*;
        assert_eq!(step(None, 5, Prev), Some(4));
        assert_eq!(step(Some(0), 5, Prev), Some(0), "stays at the start");
        assert_eq!(step(Some(3), 5, Next), Some(4));
        assert_eq!(step(Some(4), 5, Next), None, "stepping onto the last position resumes live");
        assert_eq!(step(None, 5, Next), None);
        assert_eq!(step(Some(2), 5, First), Some(0));
        assert_eq!(step(Some(2), 5, Live), None);
        assert_eq!(step(None, 5, To(1)), Some(1));
        assert_eq!(step(None, 5, To(5)), None);
        assert_eq!(step(None, 0, Prev), None, "no moves: nothing to browse");
    }

    #[test]
    fn ggf_tag_letter_does_not_pick_the_mover() {
        let rec = GameRecord::parse(
            "(;BO[8 -------- -------- -------- ---O*--- ---*O--- -------- -------- -------- *]W[d3];)",
        )
        .unwrap();
        let g = Game::from_record(".1".into(), rec, Instant::now()).unwrap();
        assert_eq!(g.moves[0].color, Color::Black);
        assert_eq!(g.board(None).cell(3, 2), Cell::Disc(Color::Black));
    }

    #[test]
    fn a_mid_game_record_replays_its_discs_side_to_move_and_clocks() {
        let rec = GameRecord::parse(
            "(;GM[Othello]PB[ann]PW[ben]TY[8]TI[10:00/5/]\
             BO[8 -------- -------- -------- ---O*--- ---*O--- -------- -------- -------- *]\
             B[f5//12]W[d6//20]B[c3//1];)",
        )
        .unwrap();
        let now = Instant::now();
        let g = Game::from_record(".7".into(), rec, now).unwrap();
        assert_eq!(g.board(None).count(), (5, 2), "tiger opening disc count");
        assert_eq!(g.board(None).to_move(), Color::White);
        assert_eq!(g.clock_secs(Color::Black, None, now), 600 + 5 - 12 + 5 - 1);
        assert_eq!(g.clock_secs(Color::White, None, now), 600 + 5 - 20);
    }

    #[test]
    fn a_record_with_an_illegal_move_is_rejected() {
        let bad = GameRecord::parse(
            "(;BO[8 -------- -------- -------- ---O*--- ---*O--- -------- -------- -------- *]B[a1];)",
        );
        assert!(Game::from_record(".8".into(), bad.unwrap(), Instant::now()).is_err(), "illegal GGF move");
    }

    #[test]
    fn a_komi_bid_is_charged_at_its_sides_first_move_and_the_other_bid_at_once() {
        let now = Instant::now();
        let later = now + Duration::from_secs(3);
        for (side, starter, tag) in [("*", Color::Black, "KW"), ("O", Color::White, "KB")] {
            let bo = format!("BO[8 -------- -------- -------- ---O*--- ---*O--- -------- -------- -------- {side}]");
            for (bid, spent) in [(format!("{tag}[x/1/7]"), 7), (format!("{tag}[x/1/0]"), 0), (String::new(), 0)] {
                let decided = GameRecord::parse(&format!("(;TY[8k]TI[5:00/2/]KM[1]{bid}{bo};)")).unwrap();
                let g = Game::from_record(".1".into(), decided, now).unwrap();
                assert_eq!(
                    g.clock_secs(starter, None, now),
                    300,
                    "the starting bid is not charged before its first move"
                );
                assert_eq!(
                    g.clock_secs(starter.other(), None, now),
                    302 - spent,
                    "the other bid receives one increment, including zero-time bids"
                );
                assert_eq!(g.clock_secs(starter, None, later), 297, "only the starting side runs once komi is decided");
                assert_eq!(g.clock_secs(starter.other(), None, later), 302 - spent, "the other side is stopped");
            }
        }
        // As GGS writes it: both bids, with the starter's bid replayed as the first move (Game.C:1160-1172).
        let bo = "BO[8 -------- -------- -------- ---O*--- ---*O--- -------- -------- -------- *]";
        let rec = GameRecord::parse(&format!("(;TY[8k]TI[5:00/2/]KB[d3/1/7]KW[x/1/4]KM[1]{bo}B[d3//7];)")).unwrap();
        let g = Game::from_record(".3".into(), rec, now).unwrap();
        assert_eq!(g.clock_secs(Color::Black, None, now), 295, "the starter's bid time and increment are charged once");
        assert_eq!(g.clock_secs(Color::White, None, now), 298);
    }

    #[test]
    fn clocks_before_komi_decision_or_normal_first_move_have_no_extra_increment() {
        const BO: &str = "BO[8 -------- -------- -------- ---O*--- ---*O--- -------- -------- -------- *]";
        let now = Instant::now();
        let later = now + Duration::from_secs(3);
        for increment in [0, 2] {
            for kind in ["8k", "8"] {
                let rec = GameRecord::parse(&format!("(;TY[{kind}]TI[5:00/{increment}/]{BO};)")).unwrap();
                let g = Game::from_record(".2".into(), rec, now).unwrap();
                assert_eq!(g.clock_secs(Color::Black, None, now), 300);
                assert_eq!(g.clock_secs(Color::White, None, now), 300);
                assert_eq!(g.clock_secs(Color::Black, None, later), 297);
                assert_eq!(g.clock_secs(Color::White, None, later), if kind == "8k" { 297 } else { 300 });
            }
        }
    }

    #[test]
    fn last_evals_keep_the_movers_perspective() {
        let g = Game::from_record(".1".into(), GameRecord::parse(SPEC_JOIN).unwrap(), Instant::now()).unwrap();
        assert_eq!(g.last_eval(Color::Black, None), Some(0.));
        assert_eq!(
            (g.stated_eval(Color::Black, None), g.stated_eval(Color::White, None)),
            (None, Some(-1.23)),
            "a player that never writes one claims nothing"
        );
        let g = Game::from_record(
            ".1".into(),
            GameRecord::parse(&SPEC_JOIN.replace("d3//3.10", "d3/2.5")).unwrap(),
            Instant::now(),
        )
        .unwrap();
        assert_eq!((g.last_eval(Color::Black, None), g.last_eval(Color::White, None)), (Some(2.5), Some(-1.23)));
        assert_eq!(g.stated_eval(Color::Black, None), Some(2.5));
        assert_eq!(g.last_eval(Color::White, Some(1)), None, "not yet played at move 1");
    }

    #[test]
    fn join_replays_moves_and_clocks() {
        let now = Instant::now();
        let lobby = joined(now);
        let g = &lobby.games[".12"];
        assert_eq!(lobby.watched, [".12"], "a join is adopted");
        assert_eq!(lobby.focus.as_deref(), Some(".12"));
        assert_eq!(g.board(None).count(), (3, 3));
        assert_eq!(g.board(None).to_move(), Color::Black);
        assert_eq!(g.moves.iter().map(|m| m.color).collect::<Vec<_>>(), [Color::Black, Color::White]);
        assert_eq!(format_secs(g.clock_secs(Color::Black, None, now)), "+0:04:56");
        assert_eq!(format_secs(g.clock_secs(Color::White, None, now)), "+0:04:55");
        let later = now + Duration::from_millis(2500);
        assert_eq!(g.clock_secs(Color::Black, None, later), 294, "side to move ticks");
        assert_eq!(g.clock_secs(Color::White, None, later), 295, "other side is frozen");
        assert_eq!(g.last_square(None).map(|s| s.to_string()).as_deref(), Some("c5"));
    }

    #[test]
    fn positions_keep_board_and_stored_clocks_per_move() {
        let now = Instant::now();
        let g = &joined(now).games[".12"];
        assert_eq!(g.board(Some(0)).count(), (2, 2));
        assert_eq!(g.board(Some(1)).count(), (4, 1), "after d3");
        assert_eq!(g.board(Some(1)).to_move(), Color::White);
        assert_eq!(g.last_square(Some(1)).map(|s| s.to_string()).as_deref(), Some("d3"));
        assert_eq!(g.last_square(Some(0)), None);
        let later = now + Duration::from_secs(9);
        assert_eq!(g.clock_secs(Color::Black, Some(1), later), 296, "stored 300 - 3.1, not ticking");
        assert_eq!(g.clock_secs(Color::White, Some(1), later), 300);
        assert!(!g.is_running(Color::White, Some(1)));
        assert_eq!(g.clock_secs(Color::Black, None, later), 296 - 9, "live side ticks");
        assert_eq!(g.board(Some(99)).count(), g.board(None).count(), "out of range clamps to live");
    }

    #[test]
    fn updates_charge_clocks_illegal_moves_are_reported_and_end_stops_the_clocks() {
        let now = Instant::now();
        let mut lobby = joined(now);
        let later = now + Duration::from_secs(10);
        assert!(lobby.apply(OsMsg::Update { id: ".12".into(), token: tok("e6//7") }, later).is_empty());
        let g = &lobby.games[".12"];
        assert_eq!(g.moves.len(), 3);
        assert_eq!(g.clock_secs(Color::Black, None, later), 300 - 3 - 7 - 1, "3.1 + 7 used; truncation of 289.9");
        assert_eq!(g.clock_secs(Color::White, None, later + Duration::from_secs(4)), 295 - 4);
        let notes = lobby.apply(OsMsg::Update { id: ".12".into(), token: tok("a1") }, later);
        assert_eq!(notes.len(), 1, "illegal move reported");
        assert_eq!(lobby.games[".12"].moves.len(), 3);
        lobby.apply(OsMsg::End { id: ".12".into(), outcome: Outcome::Aborted }, later);
        let g = &lobby.games[".12"];
        assert_eq!(g.phase, Phase::Over(Outcome::Aborted));
        assert!(g.text.is_empty(), "the result is not kibitz");
        assert_eq!(g.clock_secs(Color::White, None, later + Duration::from_secs(60)), 295, "clocks stop");
        assert!(!lobby.apply(OsMsg::Update { id: ".12".into(), token: tok("e3") }, later).is_empty());
    }

    #[test]
    fn invalid_move_times_do_not_advance_the_game() {
        let now = Instant::now();
        let mut lobby = joined(now);
        let g = lobby.games.get_mut(".12").unwrap();
        let before = g.clone();
        for time in [-1.0, f64::NAN, f64::INFINITY, 1e13] {
            assert!(g.play(MoveToken { time, ..tok("e6") }, now).is_err());
            assert_eq!(g.moves(), before.moves());
            assert_eq!(g.board(None), before.board(None));
            assert_eq!(g.positions.len(), before.positions.len());
            assert_eq!(g.positions.last().unwrap().clocks, before.positions.last().unwrap().clocks);
            assert_eq!(g.last_move_at, before.last_move_at);
            assert_eq!(g.clock_estimated, before.clock_estimated);
        }
    }
}
