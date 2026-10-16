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

/// Each login's log lines and moves, oldest first.
#[derive(Default, Debug)]
pub struct Searches {
    entries: BTreeMap<String, VecDeque<Entry>>,
    /// Entries added so far, so a view can tell something new arrived.
    added: u64,
}

/// An engine logs every move of every game it plays; older entries are dropped past this many.
const KEEP: usize = 4096;

impl Searches {
    pub fn add_line(&mut self, engine: &str, line: &str) {
        self.push(engine, Entry::Line(line.to_string()));
    }

    pub fn add_move(&mut self, player: &str, game: &str, ply: usize, mv: Move) {
        self.push(player, Entry::Move { game: game.to_string(), ply, mv });
    }

    fn push(&mut self, login: &str, entry: Entry) {
        self.added += 1;
        let entries = self.entries.entry(login.to_string()).or_default();
        entries.push_back(entry);
        if entries.len() > KEEP {
            entries.pop_front();
        }
    }

    /// `login`'s entries, oldest first.
    pub fn timeline(&self, login: &str) -> impl Iterator<Item = &Entry> {
        self.entries.get(login).into_iter().flatten()
    }

    pub fn added(&self) -> u64 {
        self.added
    }

    /// Where the `ply`th move of `game` falls in `login`'s timeline.
    pub fn position(&self, login: &str, game: &str, ply: usize) -> Option<usize> {
        self.timeline(login).position(|e| matches!(e, Entry::Move { game: g, ply: p, .. } if g == game && *p == ply))
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
        assert_eq!(s.position("e", ".3.0", 17), Some(1));
        assert_eq!(s.position("e", ".3.1", 17), None);
        assert_eq!(s.timeline("other").count(), 0);
        for _ in 0..KEEP {
            s.add_line("e", "x");
        }
        assert_eq!(s.timeline("e").count(), KEEP);
        assert_eq!(s.position("e", ".3.0", 17), None, "the oldest entries go first");
    }
}
