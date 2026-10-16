use crate::othello::Move;
use std::collections::{BTreeMap, VecDeque};

/// What happened on an engine's log, in arrival order: a line it sent to its log channel, or a move the
/// server announced it played. Engines write in their own formats and at their own times, so nothing ties a
/// line to a move but where the two fall in this order.
#[derive(Clone, PartialEq, Debug)]
pub enum Entry {
    Line(String),
    /// The `ply`th move of board `game`.
    Move {
        game: String,
        ply: usize,
        mv: Move,
    },
}

/// The channel an engine logs its searches to: `.` and the first 7 characters of its login.
pub fn channel(login: &str) -> String {
    format!(".{}", login.chars().take(7).collect::<String>())
}

/// A message on `channel` is a log when its sender owns that channel.
pub fn is_log(channel: &str, from: &str) -> bool {
    self::channel(from) == channel
}

/// Each login's log lines and moves, oldest first, numbered in the order they came across logins.
#[derive(Default, Debug)]
pub struct Searches {
    entries: BTreeMap<String, VecDeque<(u64, Entry)>>,
    /// Entries added so far: the next entry's number, and how a view tells something new arrived.
    added: u64,
}

/// A row of two players' logs side by side.
#[derive(Clone, PartialEq, Debug)]
pub enum Row {
    /// The `ply`th moves the server announced, each by its board under the player who played it. The two
    /// boards of a synchro match reach a move number one after the other, so their moves share a rule
    /// when no line came between them.
    Move { ply: usize, games: [Option<String>; 2] },
    /// What each player sent between two moves.
    Lines([Vec<String>; 2]),
}

/// Where the `ply`th move of `game` falls among `rows`.
pub fn row_of(rows: &[Row], game: &str, ply: usize) -> Option<usize> {
    // A match number comes back for a later game; the shown game is the later one.
    rows.iter().rposition(|r| match r {
        Row::Move { ply: p, games } => *p == ply && games.iter().flatten().any(|g| g == game),
        Row::Lines(_) => false,
    })
}

/// A line of dashes and nothing else.
fn is_rule(line: &str) -> bool {
    let line = line.trim();
    !line.is_empty() && line.chars().all(|c| c == '-')
}

/// An engine logs every move of every game it plays; older entries are dropped past this many.
const KEEP: usize = 4096;
/// Lines a row holds per player before the next row continues them, so a long stretch between two moves
/// is drawn a row at a time, as it scrolls into view.
const ROW_LINES: usize = 32;

impl Searches {
    pub fn add_line(&mut self, engine: &str, line: &str) {
        self.push(engine, Entry::Line(line.to_string()));
    }

    pub fn add_move(&mut self, player: &str, game: &str, ply: usize, mv: Move) {
        self.push(player, Entry::Move { game: game.to_string(), ply, mv });
    }

    fn push(&mut self, login: &str, entry: Entry) {
        let entries = self.entries.entry(login.to_string()).or_default();
        entries.push_back((self.added, entry));
        self.added += 1;
        if entries.len() > KEEP {
            entries.pop_front();
        }
    }

    /// `login`'s entries, oldest first.
    #[cfg(test)]
    pub fn timeline(&self, login: &str) -> impl Iterator<Item = &Entry> {
        self.entries.get(login).into_iter().flatten().map(|(_, e)| e)
    }

    /// Two players' logs in the order they came: each announced move is a row across both, and what the
    /// players sent between two moves shares the row between them.
    pub fn rows(&self, players: [&str; 2]) -> Vec<Row> {
        let mut merged: Vec<(u64, usize, &Entry)> = Vec::new();
        for (side, login) in players.iter().enumerate() {
            if side == 1 && players[0] == players[1] {
                break;
            }
            merged.extend(self.entries.get(*login).into_iter().flatten().map(|(n, e)| (*n, side, e)));
        }
        merged.sort_by_key(|(n, ..)| *n);
        let mut rows = Vec::new();
        for (_, side, entry) in merged {
            match entry {
                Entry::Move { game, ply, .. } => match rows.last_mut() {
                    Some(Row::Move { ply: p, games }) if p == ply && games[side].is_none() => {
                        games[side] = Some(game.clone());
                    }
                    _ => {
                        let mut games = [None, None];
                        games[side] = Some(game.clone());
                        rows.push(Row::Move { ply: *ply, games });
                    }
                },
                // A rule some engines draw between their reports; the move rules already divide them.
                Entry::Line(text) if is_rule(text) => {}
                Entry::Line(text) => {
                    if !matches!(rows.last(), Some(Row::Lines(c)) if c[side].len() < ROW_LINES) {
                        rows.push(Row::Lines([Vec::new(), Vec::new()]));
                    }
                    if let Some(Row::Lines(columns)) = rows.last_mut() {
                        columns[side].push(text.clone());
                    }
                }
            }
        }
        rows
    }

    pub fn added(&self) -> u64 {
        self.added
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_owner_of_a_channel_logs_to_it() {
        assert!(is_log(".Rhapsod", "Rhapsody"));
        assert!(is_log(".iEdax", "iEdax"));
        assert!(!is_log(".chat", "iEdax"));
        assert!(!is_log(".iEdax", "bob"));
    }

    #[test]
    fn lines_and_moves_keep_their_arrival_order() {
        let mut s = Searches::default();
        let g5 = Move::parse("g5").unwrap();
        s.add_line("e", "thinking");
        s.add_move("e", ".3.0", 17, g5);
        s.add_line("e", "  score == -2 ; PV: G5  ");
        assert_eq!(
            s.timeline("e").cloned().collect::<Vec<_>>(),
            [
                Entry::Line("thinking".into()),
                Entry::Move { game: ".3.0".into(), ply: 17, mv: g5 },
                Entry::Line("  score == -2 ; PV: G5  ".into()),
            ],
            "lines exactly as sent"
        );
        assert_eq!(s.timeline("other").count(), 0);
        for _ in 0..KEEP {
            s.add_line("e", "x");
        }
        assert_eq!(s.timeline("e").count(), KEEP);
        assert!(!s.timeline("e").any(|e| matches!(e, Entry::Move { .. })), "the oldest entries go first");
    }

    #[test]
    fn two_players_share_rows_between_the_moves() {
        let mut s = Searches::default();
        let mv = |m: &str| Move::parse(m).unwrap();
        s.add_line("a", "a thinks");
        s.add_line("b", "b waits");
        s.add_line("a", "a decides");
        s.add_move("a", ".3.0", 17, mv("g5"));
        s.add_move("b", ".3.1", 17, mv("c2"));
        s.add_line("b", "b thinks");
        s.add_line("x", "someone else");
        let rows = s.rows(["a", "b"]);
        let lines = |a: &[&str], b: &[&str]| Row::Lines([a, b].map(|c| c.iter().map(|l| l.to_string()).collect()));
        assert_eq!(
            rows,
            [
                lines(&["a thinks", "a decides"], &["b waits"]),
                Row::Move { ply: 17, games: [Some(".3.0".into()), Some(".3.1".into())] },
                lines(&[], &["b thinks"]),
            ]
        );
        assert_eq!(row_of(&rows, ".3.1", 17), Some(1), "the synchro boards' 17th moves share a rule");
        assert_eq!(row_of(&rows, ".3.1", 18), None);
        s.add_line("b", "-----------");
        s.add_move("a", ".3.0", 18, mv("a1"));
        s.add_line("a", "between");
        s.add_move("b", ".3.1", 18, mv("a2"));
        s.add_move("b", ".3.0", 19, mv("a3"));
        s.add_move("b", ".3.1", 19, mv("a4"));
        let rows = s.rows(["a", "b"]);
        let move_row =
            |a: Option<&str>, b: Option<&str>, ply| Row::Move { ply, games: [a, b].map(|g| g.map(str::to_string)) };
        assert_eq!(
            rows[2..],
            [
                lines(&[], &["b thinks"]),
                move_row(Some(".3.0"), None, 18),
                lines(&["between"], &[]),
                move_row(None, Some(".3.1"), 18),
                move_row(None, Some(".3.0"), 19),
                move_row(None, Some(".3.1"), 19),
            ],
            "a line of dashes is not shown; a line between, or the same player twice, keeps the rules apart"
        );
        assert_eq!(s.rows(["a", "a"]), s.rows(["a", "nobody"]), "one login playing itself is listed once");
        for _ in 0..=ROW_LINES {
            s.add_line("a", "a again");
        }
        let rows = s.rows(["a", "b"]);
        let [.., Row::Lines(full), Row::Lines(next)] = rows.as_slice() else { panic!("{rows:?}") };
        assert_eq!((full[0].len(), full[1].len()), (ROW_LINES, 0));
        assert_eq!((next[0].len(), next[1].len()), (1, 0), "a full row continues in the next");
    }
}
