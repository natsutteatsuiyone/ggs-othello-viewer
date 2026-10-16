use crate::chat::ChatLine;
use crate::othello::Move;
use std::collections::BTreeMap;
use std::time::Instant;

/// One message an engine sent to its log channel. Engines write in their own formats: the Edax fields are
/// read where they appear, and every other line is kept as it came.
#[derive(Clone, PartialEq, Debug)]
pub struct SearchLog {
    /// The login that sent it; in a synchro match its messages on both boards share the channel.
    pub engine: String,
    /// The board it is about, when it names one (`.15.0`).
    pub game: Option<String>,
    /// The move played, when it says `<engine> plays <move>`; a pass is not a move here.
    pub mv: Option<Move>,
    pub depth: Option<String>,
    pub score: Option<String>,
    pub pv: Option<String>,
    pub nodes: Option<String>,
    /// The search time.
    pub time: Option<String>,
    pub speed: Option<String>,
    /// Lines the fields above did not consume, shown as they are.
    pub rest: Vec<String>,
}

impl SearchLog {
    /// Whether any Edax field was found, so the report can be laid out by field.
    pub fn has_fields(&self) -> bool {
        [&self.depth, &self.score, &self.pv, &self.nodes, &self.time, &self.speed].iter().any(|f| f.is_some())
    }
}

/// The channel an engine logs its searches to: `.` and the first 7 characters of its login.
pub fn channel(login: &str) -> String {
    format!(".{}", login.chars().take(7).collect::<String>())
}

/// A message on `channel` is a log when its sender owns that channel.
pub fn is_log(channel: &str, from: &str) -> bool {
    self::channel(from) == channel
}

/// A game id such as `.15` or `.15.0`, with trailing punctuation removed. Only the word after `game` is
/// read as one: a bare `.5` may be a number.
fn game_id(word: &str) -> Option<&str> {
    let id = word.trim_end_matches([';', ',', ':', ')']).trim_end_matches('.');
    let parts = id.strip_prefix('.')?;
    parts.split('.').all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit())).then_some(id)
}

pub fn parse(line: &ChatLine) -> SearchLog {
    let mut log = SearchLog {
        engine: line.from.clone(),
        game: None,
        mv: None,
        depth: None,
        score: None,
        pv: None,
        nodes: None,
        time: None,
        speed: None,
        rest: Vec::new(),
    };
    for l in &line.lines {
        let l = l.trim();
        if l.is_empty() || l.chars().all(|c| c == '-') {
            continue;
        }
        if log.game.is_none() {
            let words: Vec<&str> = l.split_whitespace().collect();
            log.game = words.windows(2).find(|w| w[0] == "game").and_then(|w| game_id(w[1])).map(str::to_string);
        }
        let plays = l.strip_prefix(line.from.as_str()).and_then(|r| r.strip_prefix(" plays "));
        if let Some(mv) = plays.and_then(|r| r.split_whitespace().next()).and_then(|w| Move::parse(w).ok()) {
            log.mv = Some(mv).filter(|m| *m != Move::Pass);
            continue;
        }
        let mut found = false;
        for part in l.split(';').map(str::trim) {
            let field = |key: &str| part.strip_prefix(key).map(|v| v.trim().to_string());
            if let Some(v) = field("score ==") {
                let (score, depth) = v.split_once(" at ").unwrap_or((&v, ""));
                log.score = Some(score.trim().into());
                log.depth = Some(depth.trim().into()).filter(|d: &String| !d.is_empty());
            } else if let Some(v) = field("PV:") {
                log.pv = Some(v);
            } else if let Some(v) = field("nodes:") {
                log.nodes = Some(v);
            } else if let Some(v) = field("time:") {
                // `search = 10.4s, move = 4.3s`: the search time, or the whole text in another form.
                let search = v.split(',').find_map(|t| t.trim().strip_prefix("search =")).map(str::trim);
                log.time = Some(search.map_or(v.clone(), str::to_string));
            } else if let Some(v) = field("speed:") {
                log.speed = Some(v.trim_end_matches('.').to_string());
            } else {
                continue;
            }
            found = true;
        }
        if !found {
            log.rest.push(l.to_string());
        }
    }
    log
}

/// Logs kept per engine, oldest first.
#[derive(Default, Debug)]
pub struct Searches(BTreeMap<String, Vec<(Instant, SearchLog)>>);

/// An engine logs every move of every game it plays; older logs are dropped past this many.
const KEEP: usize = 512;

impl Searches {
    /// A later log on the same move of the same board replaces the earlier one.
    pub fn add(&mut self, log: SearchLog, now: Instant) {
        let logs = self.0.entry(log.engine.clone()).or_default();
        if log.game.is_some() && log.mv.is_some() {
            logs.retain(|(_, l)| (&l.game, l.mv) != (&log.game, log.mv));
        }
        logs.push((now, log));
        if logs.len() > KEEP {
            logs.drain(..logs.len() - KEEP);
        }
    }

    /// `engine`'s newest log about one of `games`, or about no board in particular.
    pub fn latest(&self, engine: &str, games: &[&str]) -> Option<&SearchLog> {
        let logs = self.0.get(engine)?;
        logs.iter().rev().map(|(_, l)| l).find(|l| l.game.as_deref().is_none_or(|g| games.contains(&g)))
    }

    /// `engine`'s log on the move `mv` of board `game`.
    pub fn on_move(&self, engine: &str, game: &str, mv: Move) -> Option<&SearchLog> {
        let logs = self.0.get(engine)?;
        logs.iter().rev().map(|(_, l)| l).find(|l| l.game.as_deref() == Some(game) && l.mv == Some(mv))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chat(from: &str, lines: &[&str]) -> ChatLine {
        ChatLine { from: from.into(), lines: lines.iter().map(|l| l.to_string()).collect() }
    }

    #[test]
    fn an_edax_report_fills_every_field() {
        let log = parse(&chat(
            "iEdax",
            &[
                "-----------------------------------------",
                "iEdax plays F2 in game .1.1 using 4 threads ;",
                "score == -2 at 38@87% ; PV: F2 e7 B6 c7 B3 c2 F8 a2  ;",
                "nodes:  2.149 GN ; time: search = 10.4s, move = 4.3s; speed:  205.7 MN/s.",
                "search out of time at 38@87%",
            ],
        ));
        assert_eq!(
            log,
            SearchLog {
                engine: "iEdax".into(),
                game: Some(".1.1".into()),
                mv: Some(Move::parse("f2").unwrap()),
                depth: Some("38@87%".into()),
                score: Some("-2".into()),
                pv: Some("F2 e7 B6 c7 B3 c2 F8 a2".into()),
                nodes: Some("2.149 GN".into()),
                time: Some("10.4s".into()),
                speed: Some("205.7 MN/s".into()),
                rest: vec!["search out of time at 38@87%".into()],
            }
        );
        assert!(log.has_fields());
    }

    #[test]
    fn another_format_keeps_its_lines_and_still_finds_the_board() {
        let log = parse(&chat("Melody", &["[.7] depth 20 eval +4 (exact)", "pv c4 c3 speed 1.2e6"]));
        assert_eq!((log.game.as_deref(), log.mv), (None, None), "only the word after `game` names a board");
        assert!(!log.has_fields());
        assert_eq!(log.rest, ["[.7] depth 20 eval +4 (exact)", "pv c4 c3 speed 1.2e6"]);
        let log = parse(&chat("Melody", &["Melody plays c4 in game .7.1:", "eval +4"]));
        assert_eq!((log.game.as_deref(), log.mv), (Some(".7.1"), Some(Move::parse("C4").unwrap())));
        assert_eq!(log.rest, ["eval +4"]);
        assert_eq!(parse(&chat("bob", &["speed .5 at 1.5"])).game, None, "a decimal is not a game id");
        assert_eq!(parse(&chat("bob", &["bob plays PA in game .4"])).mv, None, "a pass");
    }

    #[test]
    fn only_the_owner_of_a_channel_logs_to_it() {
        assert!(is_log(".Rhapsod", "Rhapsody"));
        assert!(is_log(".iEdax", "iEdax"));
        assert!(!is_log(".chat", "iEdax"));
        assert!(!is_log(".iEdax", "bob"));
    }

    #[test]
    fn the_newest_log_on_the_shown_boards_wins_and_a_move_is_logged_once() {
        let mut s = Searches::default();
        let t = Instant::now();
        let log = |text: &str| parse(&chat("e", &[text]));
        s.add(log("e plays a1 in game .3.0"), t);
        s.add(log("e plays h8 in game .3.1"), t);
        s.add(log("e plays b2 in game .9"), t);
        assert_eq!(s.latest("e", &[".3.0", ".3.1"]).unwrap().game.as_deref(), Some(".3.1"));
        s.add(log("thinking"), t);
        assert_eq!(s.latest("e", &[".3.0"]).unwrap().rest, ["thinking"], "a log naming no board counts anywhere");
        s.add(parse(&chat("e", &["e plays a1 in game .3.0", "again"])), t);
        let a1 = Move::parse("a1").unwrap();
        assert_eq!(s.on_move("e", ".3.0", a1).unwrap().rest, ["again"]);
        assert_eq!(s.0["e"].iter().filter(|(_, l)| l.mv == Some(a1)).count(), 1);
        assert!(s.on_move("e", ".3.1", a1).is_none());
        for _ in 0..KEEP {
            s.add(log("x"), t);
        }
        assert_eq!(s.0["e"].len(), KEEP);
    }
}
