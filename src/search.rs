use crate::chat::ChatLine;
use crate::othello::Move;

/// One engine's report on the move it just played, from its log channel.
#[derive(Clone, PartialEq, Debug)]
pub struct SearchLog {
    pub game: String,
    /// The square played; a pass report is ignored.
    pub mv: Move,
    pub depth: Option<String>,
    pub score: Option<String>,
    pub pv: Option<String>,
    pub nodes: Option<String>,
    /// The search time.
    pub time: Option<String>,
    pub speed: Option<String>,
    /// Lines the fields above did not consume (other engines' formats, "search out of time at ..."), shown as they are.
    pub rest: Vec<String>,
}

/// The channel an engine logs its searches to: `.` and the first 7 characters of its login.
pub fn channel(login: &str) -> String {
    format!(".{}", login.chars().take(7).collect::<String>())
}

/// A report is any message with a line `<from> plays <move> in game <.id>`; the Edax fields are read from
/// its other lines where they appear.
pub fn parse(line: &ChatLine) -> Option<SearchLog> {
    let (i, game, mv) = line.lines.iter().enumerate().find_map(|(i, l)| {
        let mut words = l.strip_prefix(line.from.as_str())?.strip_prefix(" plays ")?.split_whitespace();
        let mv = Move::parse(words.next()?).ok().filter(|m| *m != Move::Pass)?;
        let game = match (words.next(), words.next(), words.next()) {
            (Some("in"), Some("game"), Some(id)) if id.starts_with('.') => id,
            _ => return None,
        };
        Some((i, game.to_string(), mv))
    })?;
    let mut log = SearchLog {
        game,
        mv,
        depth: None,
        score: None,
        pv: None,
        nodes: None,
        time: None,
        speed: None,
        rest: Vec::new(),
    };
    for (j, l) in line.lines.iter().enumerate() {
        let l = l.trim();
        if j == i || l.is_empty() || l.chars().all(|c| c == '-') {
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
    Some(log)
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
        ))
        .unwrap();
        assert_eq!(
            log,
            SearchLog {
                game: ".1.1".into(),
                mv: Move::parse("f2").unwrap(),
                depth: Some("38@87%".into()),
                score: Some("-2".into()),
                pv: Some("F2 e7 B6 c7 B3 c2 F8 a2".into()),
                nodes: Some("2.149 GN".into()),
                time: Some("10.4s".into()),
                speed: Some("205.7 MN/s".into()),
                rest: vec!["search out of time at 38@87%".into()],
            }
        );
    }

    #[test]
    fn another_engine_keeps_its_lines_as_they_are() {
        let log =
            parse(&chat("Melody", &["Melody plays c4 in game .7", "depth 20 eval +4 (exact)", "pv c4 c3"])).unwrap();
        assert_eq!((log.game.as_str(), log.mv), (".7", Move::parse("C4").unwrap()));
        assert_eq!(
            (log.depth, log.score, log.pv, log.nodes, log.time, log.speed),
            (None, None, None, None, None, None)
        );
        assert_eq!(log.rest, ["depth 20 eval +4 (exact)", "pv c4 c3"]);
    }

    #[test]
    fn only_the_senders_own_move_report_counts() {
        assert_eq!(parse(&chat("bob", &["hello, who plays tonight?"])), None);
        assert_eq!(parse(&chat("bob", &["alice plays d3 in game .4"])), None, "someone else's move");
        assert_eq!(parse(&chat("bob", &["bob plays PA in game .4"])), None, "a pass");
        assert_eq!(channel("Rhapsody"), ".Rhapsod");
        assert_eq!(channel("iEdax"), ".iEdax");
    }
}
