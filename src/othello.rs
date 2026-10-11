use std::fmt;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Color {
    Black,
    White,
}

impl Color {
    pub fn other(self) -> Color {
        match self {
            Color::Black => Color::White,
            Color::White => Color::Black,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Cell {
    /// Outside the board, including octagon corners.
    Off,
    Empty,
    Disc(Color),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Square {
    pub x: usize,
    pub y: usize,
}

impl fmt::Display for Square {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        if self.x < 26
            && let Some(row) = self.y.checked_add(1)
        {
            write!(f, "{}{row}", char::from(b'a' + self.x as u8))
        } else {
            write!(f, "({}, {})", self.x, self.y)
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Move {
    Pass,
    Square(Square),
}

impl Move {
    pub fn parse(s: &str) -> Result<Move, String> {
        let up = s.to_ascii_uppercase();
        if matches!(up.as_str(), "PA" | "PASS" | "%") {
            return Ok(Move::Pass);
        }
        let mut chars = up.chars();
        let col = chars.next().filter(char::is_ascii_uppercase);
        let row = chars.as_str().parse::<usize>().ok().filter(|&r| r >= 1);
        match (col, row) {
            (Some(c), Some(r)) => Ok(Move::Square(Square { x: (c as u8 - b'A') as usize, y: r - 1 })),
            _ => Err(format!("bad move '{s}'")),
        }
    }
}

impl fmt::Display for Move {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Move::Pass => f.write_str("PA"),
            Move::Square(sq) => sq.fmt(f),
        }
    }
}

/// A move as the server sends it: `move[/eval[/time]]`. An empty time means 0;
/// GGS omits zero evals, so an empty eval means 0; non-finite evals are discarded.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct MoveToken {
    pub mv: Move,
    pub eval: Option<f64>,
    /// Whether the token wrote an evaluation; an omitted one reads as 0.
    pub stated: bool,
    pub time: f64,
}

/// `d3//3.1` or `c5/-1.23/4.5`: the form `parse` reads back to the same token.
impl fmt::Display for MoveToken {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}/", self.mv)?;
        if let Some(e) = self.eval.filter(|_| self.stated) {
            write!(f, "{e}")?;
        }
        write!(f, "/{}", self.time)
    }
}

impl MoveToken {
    pub fn parse(s: &str) -> Result<MoveToken, String> {
        let mut parts = s.split('/');
        let mv = Move::parse(parts.next().unwrap_or(""))?;
        let mut num = || -> Result<Option<f64>, String> {
            match parts.next().map(str::trim) {
                None | Some("") => Ok(None),
                Some(v) => v.parse().map(Some).map_err(|_| format!("bad number '{v}' in move '{s}'")),
            }
        };
        let stated = num()?;
        let eval = Some(stated.unwrap_or(0.0)).filter(|e| e.is_finite());
        let stated = stated.is_some();
        let time = num()?.unwrap_or(0.0);
        if !time.is_finite() || time < 0.0 {
            return Err(format!("invalid time '{time}' in move '{s}'"));
        }
        if parts.next().is_some() {
            return Err(format!("too many fields in move '{s}'"));
        }
        Ok(MoveToken { mv, eval, stated, time })
    }
}

const DIRS: [(isize, isize); 8] = [(-1, -1), (0, -1), (1, -1), (-1, 0), (1, 0), (-1, 1), (0, 1), (1, 1)];

const OCTAGON_OFF: [(usize, usize); 12] =
    [(0, 0), (0, 1), (1, 0), (9, 9), (9, 8), (8, 9), (0, 9), (0, 8), (1, 9), (9, 0), (8, 0), (9, 1)];

#[derive(Clone, PartialEq, Debug)]
pub struct Board {
    /// Squares along each side.
    size: usize,
    cells: Vec<Cell>,
    to_move: Color,
}

impl Board {
    /// Parses a GGF `BO[...]` value, e.g. `8 -------- ... *`.
    pub fn from_bo(value: &str) -> Result<Board, String> {
        let digits = value.chars().take_while(char::is_ascii_digit).count();
        let kind: usize = value[..digits].parse().map_err(|_| format!("bad board type in BO[{value}]"))?;
        let size = match kind {
            88 => 10,
            4..=14 => kind,
            _ => return Err(format!("unsupported board type {kind}")),
        };
        let mut cells = vec![Cell::Empty; size * size];
        if kind == 88 {
            for (x, y) in OCTAGON_OFF {
                cells[y * size + x] = Cell::Off;
            }
        }
        let mut rest = value[digits..].chars().filter(|c| !c.is_whitespace());
        for cell in cells.iter_mut().filter(|c| **c != Cell::Off) {
            *cell = match rest.next() {
                Some('*') => Cell::Disc(Color::Black),
                Some('O') => Cell::Disc(Color::White),
                Some('-') => Cell::Empty,
                Some(c) => return Err(format!("bad square '{c}' in BO")),
                None => return Err("BO has too few squares".into()),
            };
        }
        let to_move = match rest.next().map(|c| c.to_ascii_uppercase()) {
            Some('*' | 'X' | 'V') => Color::Black,
            Some('O' | 'H') => Color::White,
            _ => return Err("BO lacks side to move".into()),
        };
        if rest.next().is_some() {
            return Err("BO has trailing squares or text".into());
        }
        Ok(Board { size, cells, to_move })
    }

    pub fn size(&self) -> usize {
        self.size
    }

    pub fn to_move(&self) -> Color {
        self.to_move
    }

    /// The GGF `BO[...]` value that `from_bo` reads back to this board.
    pub fn to_bo(&self) -> String {
        let octagon = self.cells.contains(&Cell::Off);
        let mut out = if octagon { "88".to_string() } else { self.size.to_string() };
        for row in self.cells.chunks(self.size) {
            out.push(' ');
            out.extend(row.iter().filter_map(|c| match c {
                Cell::Off => None,
                Cell::Empty => Some('-'),
                Cell::Disc(Color::Black) => Some('*'),
                Cell::Disc(Color::White) => Some('O'),
            }));
        }
        out.push_str(if self.to_move == Color::Black { " *" } else { " O" });
        out
    }

    fn index(&self, x: usize, y: usize) -> usize {
        y * self.size + x
    }

    /// A square outside the board is `Cell::Off`.
    pub fn cell(&self, x: usize, y: usize) -> Cell {
        if x >= self.size || y >= self.size { Cell::Off } else { self.cells[self.index(x, y)] }
    }

    pub fn count(&self) -> (usize, usize) {
        let n = |c| self.cells.iter().filter(|&&x| x == Cell::Disc(c)).count();
        (n(Color::Black), n(Color::White))
    }

    fn flips(&self, sq: Square, color: Color) -> Vec<usize> {
        let mut out = Vec::new();
        let mut run = Vec::new();
        for (dx, dy) in DIRS {
            run.clear();
            let (mut x, mut y) = (sq.x as isize + dx, sq.y as isize + dy);
            while x >= 0 && y >= 0 && (x as usize) < self.size && (y as usize) < self.size {
                let i = self.index(x as usize, y as usize);
                match self.cells[i] {
                    Cell::Disc(c) if c != color => run.push(i),
                    Cell::Disc(_) => {
                        out.append(&mut run);
                        break;
                    }
                    _ => break,
                }
                x += dx;
                y += dy;
            }
        }
        out
    }

    /// Whether the side to move may play `sq`.
    pub fn is_legal(&self, sq: Square) -> bool {
        self.cell(sq.x, sq.y) == Cell::Empty && !self.flips(sq, self.to_move).is_empty()
    }

    fn has_legal_square(&self) -> bool {
        (0..self.size).any(|y| (0..self.size).any(|x| self.is_legal(Square { x, y })))
    }

    pub fn is_over(&self) -> bool {
        if self.has_legal_square() {
            return false;
        }
        let mut other = self.clone();
        other.to_move = self.to_move.other();
        !other.has_legal_square()
    }

    pub fn apply(&mut self, mv: Move) -> Result<(), String> {
        match mv {
            Move::Pass => {
                if self.has_legal_square() {
                    return Err("pass while a move is available".into());
                }
            }
            Move::Square(sq) => {
                if sq.x >= self.size || sq.y >= self.size || self.cell(sq.x, sq.y) != Cell::Empty {
                    return Err(format!("{sq} is not an empty square"));
                }
                let flips = self.flips(sq, self.to_move);
                if flips.is_empty() {
                    return Err(format!("{sq} flips nothing"));
                }
                for i in flips {
                    self.cells[i] = Cell::Disc(self.to_move);
                }
                let i = self.index(sq.x, sq.y);
                self.cells[i] = Cell::Disc(self.to_move);
            }
        }
        self.to_move = self.to_move.other();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const START: &str = "8 -------- -------- -------- ---O*--- ---*O--- -------- -------- -------- *";

    fn sq(s: &str) -> Move {
        Move::parse(s).unwrap()
    }

    #[test]
    fn the_start_position_puts_white_on_d4_and_black_to_move() {
        let b = Board::from_bo(START).unwrap();
        assert_eq!(b.cell(3, 3), Cell::Disc(Color::White), "d4 white");
        assert_eq!(b.cell(4, 3), Cell::Disc(Color::Black), "e4 black");
        assert_eq!(b.cell(3, 4), Cell::Disc(Color::Black), "d5 black");
        assert_eq!(b.to_move, Color::Black);
        assert_eq!(b.count(), (2, 2));
    }

    #[test]
    fn move_flips_and_toggles_side() {
        let mut b = Board::from_bo(START).unwrap();
        b.apply(sq("d3")).unwrap();
        assert_eq!(b.cell(3, 3), Cell::Disc(Color::Black), "d4 flipped");
        assert_eq!(b.count(), (4, 1));
        assert_eq!(b.to_move, Color::White);
        b.apply(sq("C5")).unwrap();
        assert_eq!(b.cell(3, 4), Cell::Disc(Color::White), "d5 flipped");
        assert_eq!(b.count(), (3, 3));
    }

    #[test]
    fn illegal_moves_rejected_without_change() {
        let mut b = Board::from_bo(START).unwrap();
        let before = b.clone();
        assert!(b.apply(sq("a1")).is_err(), "flips nothing");
        assert!(b.apply(sq("d4")).is_err(), "occupied");
        assert!(b.apply(sq("i1")).is_err(), "off board");
        assert!(b.apply(Move::Pass).is_err(), "pass with legal moves");
        assert_eq!(b, before);
    }

    #[test]
    fn legal_squares_for_side_to_move() {
        let mut b = Board::from_bo(START).unwrap();
        let legal = |b: &Board| -> Vec<String> {
            (0..8)
                .flat_map(|y| (0..8).map(move |x| Square { x, y }))
                .filter(|&s| b.is_legal(s))
                .map(|s| s.to_string())
                .collect()
        };
        assert_eq!(legal(&b), ["d3", "c4", "f5", "e6"]);
        b.apply(sq("d3")).unwrap();
        assert_eq!(legal(&b), ["c3", "e3", "c5"], "white's replies");
    }

    #[test]
    fn pass_only_when_no_move() {
        let mut passing = Board::from_bo("4 *O-- ---- ---- ---- O").unwrap();
        assert!(!passing.is_over(), "black can move after white passes");
        passing.apply(Move::Pass).unwrap();
        assert!(passing.is_legal(Square { x: 2, y: 0 }));
        let mut b = Board::from_bo("4 *O-- ---- ---- ---- *").unwrap();
        assert!(!b.is_over());
        b.apply(sq("c1")).unwrap();
        assert_eq!(b.count(), (3, 0));
        assert!(b.is_over(), "neither side can move, even with empty squares");
        assert!(b.apply(sq("d1")).is_err(), "white has no discs to flip with");
        b.apply(Move::Pass).unwrap();
        assert_eq!(b.to_move, Color::Black);
    }

    #[test]
    fn octagon_skips_corners() {
        let squares = "-".repeat(88);
        let b = Board::from_bo(&format!("88 {squares} O")).unwrap();
        assert_eq!((b.size, b.cell(0, 0), b.cell(2, 0)), (10, Cell::Off, Cell::Empty));
        assert_eq!(b.to_move, Color::White);
        assert!(Board::from_bo(&format!("88 {} O", "-".repeat(87))).is_err());
    }

    #[test]
    fn bo_text_round_trips() {
        for bo in [START, "4 *O-- ---- ---- ---- O"] {
            let b = Board::from_bo(bo).unwrap();
            assert_eq!(b.to_bo(), bo);
        }
        let octagon = Board::from_bo(&format!("88 {}O{} *", "-".repeat(40), "-".repeat(47))).unwrap();
        assert_eq!(Board::from_bo(&octagon.to_bo()).unwrap(), octagon);
        assert!(octagon.to_bo().starts_with("88 ------ "), "octagon rows skip the cut corners");
    }

    #[test]
    fn move_tokens_drop_non_finite_evals_and_remember_whether_an_eval_was_stated() {
        for eval in ["inf", "-inf", "NaN", "1e309"] {
            let token = MoveToken::parse(&format!("d3/{eval}/3.1")).unwrap();
            assert_eq!(token.mv, sq("d3"));
            assert_eq!(token.eval, None, "non-finite evals must not enter the game");
            assert_eq!(token.time, 3.1);
        }
        assert_eq!(MoveToken::parse("d3/1e9").unwrap().eval, Some(1e9));
        let t = MoveToken::parse("d3//3.10").unwrap();
        assert_eq!((t.mv, t.eval, t.time), (sq("d3"), Some(0.), 3.1));
        for s in ["d3", "d3/", "d3/0", "d3//1", "d3/0/1"] {
            assert_eq!(MoveToken::parse(s).unwrap().eval, Some(0.), "{s}");
        }
        let t = MoveToken::parse("c5/-1.23/4.5").unwrap();
        assert_eq!((t.eval, t.time), (Some(-1.23), 4.5));
        assert_eq!(
            ["d3//3.10", "d3/", "d3", "d3/0", "c5/-1.23/4.5"].map(|s| MoveToken::parse(s).unwrap().stated),
            [false, false, false, true, true]
        );
        assert_eq!(MoveToken::parse("d3//3.10").unwrap().to_string(), "d3//3.1", "an omitted evaluation stays omitted");
        assert_eq!(MoveToken::parse("pass").unwrap().mv, Move::Pass);
        assert_eq!(sq("a10").to_string(), "a10");
        assert!(MoveToken::parse("d3/x").is_err());
        assert!(MoveToken::parse("").is_err());
        for s in ["d3/0/3.1", "c5/-1.23/4.5", "PA/0/0", "f4/0.5/0"] {
            assert_eq!(MoveToken::parse(s).unwrap().to_string(), s);
        }
    }

    #[test]
    fn invalid_times_and_extra_fields_are_rejected() {
        for s in ["d3//-1", "d3//NaN", "d3//inf", "d3//1e309", "d3/0/1/2"] {
            assert!(MoveToken::parse(s).is_err(), "{s}");
        }
    }

    #[test]
    fn out_of_bounds_squares_are_off_and_cannot_change_the_board() {
        let mut b = Board::from_bo(START).unwrap();
        let before = b.clone();
        for sq in [Square { x: 8, y: 0 }, Square { x: 0, y: 8 }, Square { x: usize::MAX, y: usize::MAX }] {
            assert_eq!(b.cell(sq.x, sq.y), Cell::Off);
            assert!(!b.is_legal(sq));
            assert!(b.apply(Move::Square(sq)).is_err());
            assert_eq!(b, before);
        }
        assert!(Board::from_bo(&format!("{START} extra")).is_err());
    }
}
