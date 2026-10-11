use crate::clock::ClockSetting;
use crate::othello::{Board, Color, MoveToken};

#[derive(Clone, PartialEq, Debug, Default)]
pub struct Player {
    pub name: String,
    pub rating: f64,
}

#[derive(Clone, PartialEq, Debug)]
pub struct GameRecord {
    pub black: Player,
    pub white: Player,
    pub game_type: String,
    /// `DT` as supplied by the server.
    pub date: Option<String>,
    /// Indexed by `Color as usize`.
    pub clocks: [ClockSetting; 2],
    pub komi: Option<f64>,
    /// Seconds already spent on komi bids, charged before the first move.
    pub komi_time: [f64; 2],
    /// The `KB`/`KW` bids as written, indexed by `Color as usize`.
    pub komi_bids: [Option<String>; 2],
    /// `RE`: black's final disc margin, or `None` for `RE[?]` (Game.C `write_ggf`).
    pub result: Option<f64>,
    /// The suffix of `RE`, e.g. `r`, `s` or `t`.
    pub result_reason: Option<String>,
    pub start: Board,
    /// `B[]` and `W[]` alike; the mover is always the side to move.
    pub moves: Vec<MoveToken>,
}

fn properties(text: &str) -> Result<Vec<(String, &str)>, String> {
    let start = text.find("(;").ok_or("no '(;' in GGF")?;
    let mut rest = &text[start + 2..];
    let mut out = Vec::new();
    loop {
        rest = rest.trim_start_matches(|c: char| c.is_whitespace() || c == ';');
        if rest.starts_with(')') {
            return Ok(out);
        }
        if rest.is_empty() {
            return Err("unterminated GGF record".into());
        }
        let open = rest.find('[').ok_or_else(|| format!("property without value near '{rest}'"))?;
        let close = rest[open..].find(']').ok_or("unterminated property value")? + open;
        out.push((rest[..open].trim().to_ascii_uppercase(), &rest[open + 1..close]));
        rest = &rest[close + 1..];
    }
}

fn float(name: &str, v: &str) -> Result<f64, String> {
    v.trim().parse::<f64>().ok().filter(|n| n.is_finite()).ok_or_else(|| format!("{name}[{v}] is not a finite number"))
}

/// Time field of a komi bid `move/komi/time`.
fn bid_time(v: Option<&str>) -> Result<f64, String> {
    match v.and_then(|v| v.split('/').nth(2)).map(str::trim) {
        None | Some("") => Ok(0.0),
        Some(t) => {
            let seconds = float("K?", t)?;
            if seconds < 0.0 {
                return Err(format!("negative komi bid time '{t}'"));
            }
            Ok(seconds)
        }
    }
}

impl GameRecord {
    /// Unknown properties are skipped so that newer server fields do not break viewing.
    pub fn parse(text: &str) -> Result<GameRecord, String> {
        let mut black = Player::default();
        let mut white = Player::default();
        let mut game_type = String::new();
        let mut date = None;
        let mut clocks = [ClockSetting::default(); 2];
        let mut komi = None;
        let (mut kb, mut kw) = (None, None);
        let mut start = None;
        let mut moves = Vec::new();
        let mut result = None;
        let mut result_reason = None;
        for (name, v) in properties(text)? {
            match name.as_str() {
                "PB" => black.name = v.to_string(),
                "PW" => white.name = v.to_string(),
                "RB" => black.rating = float("RB", v)?,
                "RW" => white.rating = float("RW", v)?,
                "TY" => game_type = v.to_string(),
                "DT" => date = Some(v.to_string()),
                "TI" => clocks = [ClockSetting::parse(v)?; 2],
                "TB" => clocks[Color::Black as usize] = ClockSetting::parse(v)?,
                "TW" => clocks[Color::White as usize] = ClockSetting::parse(v)?,
                "KM" => komi = Some(float("KM", v)?),
                "KB" => kb = Some(v),
                "KW" => kw = Some(v),
                // `+64.000`, or with `:r`, `:s` or `:t` for a resignation, agreed score or timeout.
                "RE" => {
                    let (score, reason) =
                        v.split_once(':').map_or((v, None), |(score, reason)| (score, Some(reason.to_string())));
                    let score = score.trim();
                    result = if score == "?" { None } else { Some(float("RE", score)?) };
                    result_reason = reason;
                }
                "BO" if start.is_some() => return Err("second BO".into()),
                "BO" => start = Some(Board::from_bo(v)?),
                "B" | "W" if start.is_none() => return Err("move before BO".into()),
                "B" | "W" => moves.push(MoveToken::parse(v)?),
                _ => {}
            }
        }
        let start = start.ok_or("GGF has no BO")?;
        let mut komi_time = [0.0; 2];
        if komi.is_some() {
            match start.to_move() {
                Color::White => komi_time[Color::Black as usize] = bid_time(kb)?,
                Color::Black => komi_time[Color::White as usize] = bid_time(kw)?,
            }
        }
        let komi_bids = [kb, kw].map(|b| b.map(str::to_string));
        Ok(GameRecord {
            black,
            white,
            game_type,
            date,
            clocks,
            komi,
            komi_time,
            komi_bids,
            result,
            result_reason,
            start,
            moves,
        })
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::othello::Move;

    pub const SPEC_JOIN: &str = "(;GM[Othello]PC[GGS/os]DT[2006.08.17_12:00:00.GMT]PB[alice]PW[bob]RB[1850.00]RW[1720.50] \
        TI[05:00//02:00]TY[8]RE[?]BO[8 -------- -------- -------- ---O*--- ---*O--- -------- -------- -------- *] \
        B[d3//3.10]W[c5/-1.23/4.5];) ";

    #[test]
    fn the_spec_join_example_parses_players_clocks_moves_and_result() {
        let g = GameRecord::parse(SPEC_JOIN).unwrap();
        assert_eq!((g.black.name.as_str(), g.black.rating), ("alice", 1850.0));
        assert_eq!((g.white.name.as_str(), g.white.rating), ("bob", 1720.5));
        assert_eq!(g.game_type, "8");
        assert_eq!(g.clocks, [ClockSetting { initial: 300, increment: 0, extension: 120 }; 2]);
        assert_eq!(g.start.to_move(), Color::Black);
        assert_eq!(g.moves.len(), 2);
        assert_eq!(g.moves[1].mv, Move::parse("c5").unwrap());
        assert_eq!((g.moves[1].eval, g.moves[1].time), (Some(-1.23), 4.5));
        assert_eq!(g.komi, None);
        assert_eq!(g.result, None, "RE[?]");
        assert_eq!(GameRecord::parse("(;RE[-12.000:r]BO[4 ---- -O*- -*O- ---- *];)").unwrap().result, Some(-12.0));
    }

    #[test]
    fn tolerates_unknown_and_long_property_names() {
        let g = GameRecord::parse(
            "junk (;GM[Othello]COPYRIGHT[x]ZZ[y]TB[1:00]TW[2:00/3/]BO[4 ---- -O*- -*O- ---- O]\nW[a1]B[b2];)",
        )
        .unwrap();
        assert_eq!(g.clocks[0], ClockSetting { initial: 60, increment: 0, extension: 120 });
        assert_eq!(g.clocks[1], ClockSetting { initial: 120, increment: 3, extension: 120 });
        assert_eq!(g.start.to_move(), Color::White);
        assert_eq!(g.moves.len(), 2);
    }

    #[test]
    fn komi_bid_time_charged_to_non_starting_side() {
        let g = GameRecord::parse(
            "(;TY[8k]KM[-2]KB[x/1/5.5]KW[x/-2/7]BO[8 -------- -------- -------- ---O*--- ---*O--- -------- -------- -------- *];)",
        )
        .unwrap();
        assert_eq!(g.komi, Some(-2.0));
        assert_eq!(g.komi_time, [0.0, 7.0]);
        let g = GameRecord::parse("(;KM[0]BO[4 ---- -O*- -*O- ---- *];)").unwrap();
        assert_eq!(g.komi_time, [0.0, 0.0], "missing KW counts as zero");
    }

    #[test]
    fn records_with_a_misplaced_or_missing_board_or_bracket_are_rejected() {
        assert!(GameRecord::parse("(;PB[a]B[d3]BO[8 x];)").is_err(), "move before BO");
        assert!(GameRecord::parse("(;PB[a];)").is_err(), "no BO");
        assert!(GameRecord::parse("(;PB[a").is_err(), "unterminated");
        assert!(GameRecord::parse("(;BO[4 ---- -O*- -*O- ---- *]").is_err(), "missing closing parenthesis");
    }

    #[test]
    fn invalid_numbers_are_rejected() {
        let bo = "BO[4 ---- -O*- -*O- ---- *]";
        for property in [
            "RB[strong]",
            "RB[NaN]",
            "RW[inf]",
            "KM[-inf]",
            "RE[NaN]",
            "RE[bad]",
            "KM[0]KW[x/1/-1]",
            "KM[0]KW[x/1/NaN]",
        ] {
            assert!(GameRecord::parse(&format!("(;{property}{bo};)")).is_err(), "{property}");
        }
    }
}
