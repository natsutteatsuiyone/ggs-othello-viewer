use crate::othello::Move;
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

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

/// A rule such as `-----` that some engines put between reports.
fn is_separator(line: &str) -> bool {
    line.chars().all(|c| matches!(c, '-' | '=' | '*'))
}

/// `Some` for a line `<engine> plays <move>`, with the move unless it is a pass.
fn plays(engine: &str, line: &str) -> Option<Option<Move>> {
    let word = line.strip_prefix(engine)?.strip_prefix(" plays ")?.split_whitespace().next()?;
    let mv = Move::parse(word).ok()?;
    Some(Some(mv).filter(|m| *m != Move::Pass))
}

/// One report from its lines, in the order they came.
fn parse(engine: &str, lines: &[String]) -> SearchLog {
    let mut log = SearchLog {
        engine: engine.into(),
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
    for l in lines {
        if log.game.is_none() {
            let words: Vec<&str> = l.split_whitespace().collect();
            log.game = words.windows(2).find(|w| w[0] == "game").and_then(|w| game_id(w[1])).map(str::to_string);
        }
        if let Some(mv) = plays(engine, l) {
            log.mv = mv;
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

struct Report {
    /// When its last line came.
    at: Instant,
    lines: Vec<String>,
    log: SearchLog,
}

#[derive(Default)]
struct Stream {
    reports: Vec<Report>,
    /// A separator came, so the next line starts a report.
    closed: bool,
}

/// Each engine's reports, oldest first. Engines send a report as one multi-line message or one line at a
/// time, so lines are gathered into reports by what ends one: a separator, a second `plays` line, or a pause.
#[derive(Default)]
pub struct Searches(BTreeMap<String, Stream>);

impl std::fmt::Debug for Searches {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.debug_map()
            .entries(self.0.iter().map(|(e, s)| (e, s.reports.iter().map(|r| &r.log).collect::<Vec<_>>())))
            .finish()
    }
}

/// An engine logs every move of every game it plays; older reports are dropped past this many.
const KEEP: usize = 512;
/// A report that never ends, such as a running search's progress, keeps only its latest lines.
const REPORT_LINES: usize = 64;
// ponytail: a fixed pause splits reports; an engine logging slower than this within one report gets split.
const PAUSE: Duration = Duration::from_secs(2);

impl Searches {
    /// One line from `engine`'s log channel. A later report on the same move of the same board replaces the
    /// earlier one.
    pub fn add(&mut self, engine: &str, line: &str, now: Instant) {
        let line = line.trim();
        if line.is_empty() {
            return;
        }
        let stream = self.0.entry(engine.to_string()).or_default();
        if is_separator(line) {
            stream.closed = true;
            return;
        }
        let starts = match stream.reports.last() {
            None => true,
            Some(last) => {
                stream.closed
                    || now.saturating_duration_since(last.at) >= PAUSE
                    || (plays(engine, line).is_some() && last.lines.iter().any(|l| plays(engine, l).is_some()))
            }
        };
        if starts {
            stream.reports.push(Report { at: now, lines: Vec::new(), log: parse(engine, &[]) });
            stream.closed = false;
        }
        let mut report = stream.reports.pop().expect("a report is open");
        report.at = now;
        report.lines.push(line.to_string());
        if report.lines.len() > REPORT_LINES {
            report.lines.remove(0);
        }
        report.log = parse(engine, &report.lines);
        if report.log.game.is_some() && report.log.mv.is_some() {
            let id = (&report.log.game, report.log.mv);
            stream.reports.retain(|r| (&r.log.game, r.log.mv) != id);
        }
        stream.reports.push(report);
        if stream.reports.len() > KEEP {
            stream.reports.drain(..stream.reports.len() - KEEP);
        }
    }

    fn logs<'a>(&'a self, engine: &str) -> impl Iterator<Item = &'a SearchLog> + use<'a> {
        self.0.get(engine).into_iter().flat_map(|s| s.reports.iter().rev().map(|r| &r.log))
    }

    /// `engine`'s reports about one of `games`, or about no board in particular, newest first.
    pub fn history<'a>(&'a self, engine: &str, games: &'a [&str]) -> impl Iterator<Item = &'a SearchLog> + use<'a> {
        self.logs(engine).filter(|l| l.game.as_deref().is_none_or(|g| games.contains(&g)))
    }

    #[cfg(test)]
    fn latest<'a>(&'a self, engine: &str, games: &'a [&str]) -> Option<&'a SearchLog> {
        self.history(engine, games).next()
    }

    /// `engine`'s report on the move `mv` of board `game`.
    pub fn on_move(&self, engine: &str, game: &str, mv: Move) -> Option<&SearchLog> {
        self.logs(engine).find(|l| l.game.as_deref() == Some(game) && l.mv == Some(mv))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EDAX: [&str; 5] = [
        "-----------------------------------------",
        "iEdax plays F2 in game .1.1 using 4 threads ;",
        "score == -2 at 38@87% ; PV: F2 e7 B6 c7 B3 c2 F8 a2  ;",
        "nodes:  2.149 GN ; time: search = 10.4s, move = 4.3s; speed:  205.7 MN/s.",
        "search out of time at 38@87%",
    ];

    fn feed(s: &mut Searches, engine: &str, lines: &[&str], at: Instant) {
        for l in lines {
            s.add(engine, l, at);
        }
    }

    #[test]
    fn an_edax_report_fills_every_field_whether_it_comes_at_once_or_line_by_line() {
        let t = Instant::now();
        let expected = SearchLog {
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
        };
        let mut at_once = Searches::default();
        feed(&mut at_once, "iEdax", &EDAX, t);
        assert_eq!(at_once.latest("iEdax", &[".1.1"]), Some(&expected));
        let mut by_line = Searches::default();
        for (i, l) in EDAX.iter().enumerate() {
            by_line.add("iEdax", l, t + Duration::from_millis(100 * i as u64));
        }
        assert_eq!(by_line.latest("iEdax", &[".1.1"]), Some(&expected), "lines a moment apart are one report");
        assert!(expected.has_fields());
    }

    #[test]
    fn a_separator_a_second_plays_line_or_a_pause_starts_the_next_report() {
        let t = Instant::now();
        let mut s = Searches::default();
        feed(&mut s, "e", &["e plays a1 in game .3", "eval +1", "e plays b2 in game .3", "eval +2"], t);
        feed(&mut s, "e", &["----", "e plays c3 in game .3", "----", "eval +3"], t);
        s.add("e", "thinking", t + PAUSE);
        let rest = |mv: &str| s.on_move("e", ".3", Move::parse(mv).unwrap()).map(|l| l.rest.clone());
        assert_eq!(rest("a1"), Some(vec!["eval +1".to_string()]));
        assert_eq!(rest("b2"), Some(vec!["eval +2".to_string()]));
        assert_eq!(rest("c3"), Some(vec![]), "a separator ends a report even without a pause");
        assert_eq!(s.latest("e", &[".3"]).unwrap().rest, ["thinking"]);
        assert_eq!(s.0["e"].reports.len(), 5, "eval +3 after the separator stands alone");
    }

    #[test]
    fn another_format_keeps_its_lines_and_still_finds_the_board() {
        let t = Instant::now();
        let mut s = Searches::default();
        feed(&mut s, "Melody", &["[.7] depth 20 eval +4 (exact)", "pv c4 c3 speed 1.2e6"], t);
        let log = s.latest("Melody", &[]).unwrap();
        assert_eq!((log.game.as_deref(), log.mv), (None, None), "only the word after `game` names a board");
        assert!(!log.has_fields());
        assert_eq!(log.rest, ["[.7] depth 20 eval +4 (exact)", "pv c4 c3 speed 1.2e6"]);
        feed(&mut s, "Melody", &["Melody plays c4 in game .7.1:", "eval +4"], t + PAUSE);
        let log = s.latest("Melody", &[".7.1"]).unwrap();
        assert_eq!((log.game.as_deref(), log.mv), (Some(".7.1"), Some(Move::parse("C4").unwrap())));
        assert_eq!(log.rest, ["eval +4"]);
        let one = |line: &str| parse("bob", &[line.to_string()]);
        assert_eq!(one("speed .5 at 1.5").game, None, "a decimal is not a game id");
        assert_eq!(one("bob plays PA in game .4").mv, None, "a pass");
    }

    #[test]
    fn only_the_owner_of_a_channel_logs_to_it() {
        assert!(is_log(".Rhapsod", "Rhapsody"));
        assert!(is_log(".iEdax", "iEdax"));
        assert!(!is_log(".chat", "iEdax"));
        assert!(!is_log(".iEdax", "bob"));
    }

    #[test]
    fn the_newest_report_on_the_shown_boards_wins_and_a_move_is_reported_once() {
        let mut s = Searches::default();
        let mut t = Instant::now();
        let mut report = |s: &mut Searches, lines: &[&str]| {
            t += PAUSE;
            feed(s, "e", lines, t);
        };
        report(&mut s, &["e plays a1 in game .3.0"]);
        report(&mut s, &["e plays h8 in game .3.1"]);
        report(&mut s, &["e plays b2 in game .9"]);
        assert_eq!(s.latest("e", &[".3.0", ".3.1"]).unwrap().game.as_deref(), Some(".3.1"));
        report(&mut s, &["thinking"]);
        assert_eq!(s.latest("e", &[".3.0"]).unwrap().rest, ["thinking"], "a report naming no board counts anywhere");
        report(&mut s, &["e plays a1 in game .3.0", "again"]);
        let a1 = Move::parse("a1").unwrap();
        assert_eq!(s.on_move("e", ".3.0", a1).unwrap().rest, ["again"]);
        assert_eq!(s.0["e"].reports.iter().filter(|r| r.log.mv == Some(a1)).count(), 1);
        assert!(s.on_move("e", ".3.1", a1).is_none());
        for _ in 0..KEEP {
            report(&mut s, &["x"]);
        }
        assert_eq!(s.0["e"].reports.len(), KEEP);
        for i in 0..REPORT_LINES + 5 {
            s.add("e", &format!("depth {i}"), t);
        }
        assert_eq!(s.latest("e", &[]).unwrap().rest.len(), REPORT_LINES);
    }
}
