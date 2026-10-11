use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

/// The login the tournament director speaks as.
pub const TD: &str = "/td";

// ponytail: the standings table is dense, players² cells and players × rounds; caps far past any real
// field keep a bogus reply from allocating gigabytes. Virtualise the table if a real field ever nears them.
const MAX_ROUNDS: u32 = 1000;
const MAX_PLAYERS: usize = 1000;

pub fn tell(cmd: &str) -> String {
    format!("tell {TD} {cmd}")
}

/// A `tourneys` row.
#[derive(Clone, PartialEq, Debug)]
pub struct Summary {
    pub id: String,
    /// Counts the swing filler (`(bye)`) once /td has added it.
    pub players: u32,
    pub played: u32,
    pub rounds: u32,
    pub start: Start,
    pub clock: String,
    pub game_type: String,
}

#[derive(Clone, PartialEq, Debug)]
pub enum Start {
    Now,
    Done,
    /// The countdown to the next round, `m:ss` or `h:mm:ss`: round 1, or the next one during a break.
    In(String),
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Style {
    RoundRobin,
    Swiss,
}

/// A `finger <id>` reply.
#[derive(Clone, PartialEq, Debug)]
pub struct Info {
    pub id: String,
    pub played: u32,
    pub rounds: u32,
    pub style: Style,
    pub clock: String,
    pub game_type: String,
    /// Without the parenthesized filler that evens the field, such as `(bye)`.
    pub players: Vec<String>,
    /// `round 1 begins in  1:46`, `round 2 is being played`, `tournament has finished`.
    pub status: String,
}

impl Info {
    pub fn started(&self) -> bool {
        self.played > 0 || !self.status.starts_with("round 1 begins")
    }
}

#[derive(Clone, PartialEq, Debug)]
pub struct Pairing {
    pub round: u32,
    pub board: u32,
    pub black: String,
    /// `None` when `black` rests this round.
    pub white: Option<String>,
    pub result: Outcome,
}

#[derive(Clone, PartialEq, Debug)]
pub enum Outcome {
    /// The disc margin from black's side.
    Margin {
        black: f64,
        ending: Ending,
    },
    Playing,
    Unstarted,
    Bye,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Ending {
    Normal,
    Resigned,
    Timeout,
}

/// A `rankings` row; /td sends them best first.
#[derive(Clone, PartialEq, Debug)]
pub struct Standing {
    pub points: f64,
    pub wins: u32,
    pub draws: u32,
    pub losses: u32,
    /// The average disc margin.
    pub discs: f64,
    pub login: String,
    pub tiebreak: f64,
}

#[derive(Clone, PartialEq, Debug)]
pub enum TdMsg {
    Tourneys(Vec<Summary>),
    Info(Info),
    /// `st` (every listed round) or `sr` (one round).
    Schedule {
        id: String,
        pairings: Vec<Pairing>,
    },
    Rankings {
        id: String,
        standings: Vec<Standing>,
    },
    Error {
        code: u32,
        command: String,
        text: String,
    },
    Unknown(Vec<String>),
}

/// What /td has told this login, by the tournament id each reply names.
#[derive(Default, Debug)]
pub struct Tourneys {
    pub list: Option<Vec<Summary>>,
    info: BTreeMap<String, Info>,
    pub schedules: BTreeMap<String, Vec<Pairing>>,
    pub standings: BTreeMap<String, Vec<Standing>>,
    /// Each tournament's countdown to its next round from the newest reply of either kind, so the list
    /// and finger agree.
    next_round: BTreeMap<String, (Instant, String)>,
}

impl Tourneys {
    pub fn apply(&mut self, msg: TdMsg, now: Instant) {
        match msg {
            TdMsg::Tourneys(mut list) => {
                // Newest first; /td numbers tournaments in creation order.
                list.sort_by_key(|s| std::cmp::Reverse(s.id.parse::<u64>().ok()));
                for s in &list {
                    let t = match &s.start {
                        Start::In(t) => Some(t.as_str()),
                        _ => None,
                    };
                    self.set_next_round(&s.id, t, now);
                }
                self.list = Some(list);
            }
            TdMsg::Info(info) => {
                let t = info.status.rsplit_once(" in ").map(|(_, t)| t).filter(|t| countdown(t).is_some());
                self.set_next_round(&info.id, t, now);
                self.info.insert(info.id.clone(), info);
            }
            TdMsg::Schedule { id, pairings } => {
                let schedule = self.schedules.entry(id).or_default();
                let rounds: BTreeSet<u32> = pairings.iter().map(|p| p.round).collect();
                schedule.retain(|p| !rounds.contains(&p.round));
                schedule.extend(pairings);
                schedule.sort_by_key(|p| (p.round, p.board));
            }
            TdMsg::Rankings { id, standings } => {
                self.standings.insert(id, standings);
            }
            TdMsg::Error { .. } | TdMsg::Unknown(_) => {}
        }
    }

    fn set_next_round(&mut self, id: &str, t: Option<&str>, now: Instant) {
        match t {
            Some(t) => self.next_round.insert(id.to_string(), (now, t.to_string())),
            None => self.next_round.remove(id),
        };
    }

    /// What is left until `id`'s next round at `now`; `None` once it has run out.
    fn left(&self, id: &str, now: Instant) -> Option<String> {
        let (at, t) = self.next_round.get(id)?;
        remaining(t, *at, now)
    }

    /// Whether a finger or tourneys reply has shown `id` started.
    pub fn started(&self, id: &str) -> bool {
        self.info(id).is_some_and(Info::started)
            || self.rows().iter().any(|s| s.id == id && (s.played > 0 || !matches!(s.start, Start::In(_))))
    }

    pub fn rows(&self) -> &[Summary] {
        self.list.as_deref().unwrap_or_default()
    }

    pub fn info(&self, id: &str) -> Option<&Info> {
        self.info.get(id)
    }

    pub fn summary(&self, id: &str) -> Option<&Summary> {
        self.rows().iter().find(|s| s.id == id)
    }

    /// Whether finger or the list shows the end.
    pub fn finished(&self, id: &str) -> bool {
        self.summary(id).is_some_and(|s| s.start == Start::Done)
            || self.info(id).is_some_and(|i| i.status == "tournament has finished")
    }

    /// Finger's status with its countdown run down to `now`.
    pub fn status(&self, id: &str, now: Instant) -> Option<String> {
        let info = self.info.get(id)?;
        Some(match info.status.rsplit_once(" in ") {
            Some((head, t)) if countdown(t).is_some() => match self.left(id, now) {
                Some(left) => format!("{head} in {left}"),
                None => format!("{} starting...", head.trim_end_matches(" begins")),
            },
            _ => info.status.clone(),
        })
    }

    /// The real players, best first by the rankings, else in finger's order.
    pub fn players(&self, id: &str) -> Vec<String> {
        let ranked =
            self.standings.get(id).filter(|s| !s.is_empty()).map(|s| s.iter().map(|s| s.login.clone()).collect());
        ranked
            .or_else(|| self.info(id).map(|i| i.players.clone()))
            .unwrap_or_default()
            .into_iter()
            .filter(|p| !p.starts_with('('))
            .take(MAX_PLAYERS)
            .collect()
    }

    /// How many play in a `tourneys` row: finger's players without the filler once known, else the
    /// row's own count, which takes in the filler of an odd field.
    pub fn player_count(&self, s: &Summary) -> usize {
        match self.info(&s.id) {
            Some(i) => i.players.iter().filter(|p| !p.starts_with('(')).count(),
            None => s.players as usize,
        }
    }

    /// A `tourneys` row's progress at `now`; none once finished.
    pub fn progress(&self, s: &Summary, now: Instant) -> Option<String> {
        let left = || self.left(&s.id, now).map_or_else(|| "starting...".into(), |left| format!("in {left}"));
        Some(match &s.start {
            Start::In(_) if s.played == 0 => match self.left(&s.id, now) {
                Some(left) => format!("starts in {left}"),
                None => "starting...".into(),
            },
            Start::In(_) => format!("round {} of {} {}", s.played + 1, s.rounds, left()),
            Start::Now => format!("round {} of {}", (s.played + 1).min(s.rounds), s.rounds),
            Start::Done => return None,
        })
    }
}

/// One game in a crosstable cell, from the row player's side.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum CellGame {
    Margin(f64),
    Playing,
    Unstarted,
}

/// A pairing's game from black's side and from white's; `None` for a bye.
fn cells(result: &Outcome) -> Option<(CellGame, CellGame)> {
    Some(match *result {
        Outcome::Margin { black, .. } => (CellGame::Margin(black), CellGame::Margin(0. - black)),
        Outcome::Playing => (CellGame::Playing, CellGame::Playing),
        Outcome::Unstarted => (CellGame::Unstarted, CellGame::Unstarted),
        Outcome::Bye => return None,
    })
}

/// `table[r][c]`: the games between `players[r]` and `players[c]` in schedule order, from `r`'s side.
pub fn crosstable(players: &[String], pairings: &[Pairing]) -> Vec<Vec<Vec<CellGame>>> {
    let index = |name: &str| players.iter().position(|p| p == name);
    let mut table = vec![vec![Vec::new(); players.len()]; players.len()];
    for p in pairings {
        let (Some(b), Some(w)) = (index(&p.black), p.white.as_deref().and_then(index)) else { continue };
        let Some((black, white)) = cells(&p.result) else { continue };
        table[b][w].push(black);
        table[w][b].push(white);
    }
    table
}

/// The count of decided games that every meeting with any shares, so the cells can leave it out.
pub fn shared_decided(table: &[Vec<Vec<CellGame>>]) -> Option<usize> {
    let mut counts = table
        .iter()
        .flatten()
        .map(|games| games.iter().filter(|g| matches!(g, CellGame::Margin(_))).count())
        .filter(|&n| n > 0);
    let first = counts.next()?;
    counts.all(|n| n == first).then_some(first)
}

/// One round of a Swiss row, from the row player's side.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum RoundCell {
    /// A rest or a game against the filler, which /td counts a win.
    Bye,
    /// `opponent` indexes the players.
    Met { opponent: usize, game: CellGame },
}

/// `table[r][k]`: what `players[r]` had in round `k + 1` of `rounds`; `None` while unpaired.
pub fn rounds_table(players: &[String], pairings: &[Pairing], rounds: u32) -> Vec<Vec<Option<RoundCell>>> {
    let index = |name: &str| players.iter().position(|p| p == name);
    let mut table = vec![vec![None; rounds as usize]; players.len()];
    for p in pairings.iter().filter(|p| (1..=rounds).contains(&p.round)) {
        let k = p.round as usize - 1;
        let (b, w) = (index(&p.black), p.white.as_deref().and_then(index));
        let Some((black, white)) = cells(&p.result) else {
            for me in [b, w].into_iter().flatten() {
                table[me][k] = Some(RoundCell::Bye);
            }
            continue;
        };
        if let (Some(b), Some(w)) = (b, w) {
            table[b][k] = Some(RoundCell::Met { opponent: w, game: black });
            table[w][k] = Some(RoundCell::Met { opponent: b, game: white });
        }
    }
    table
}

/// `1:46` or `1:02:03`.
pub fn countdown(text: &str) -> Option<Duration> {
    let parts: Vec<u64> = text.trim().split(':').map(|p| p.parse().ok()).collect::<Option<_>>()?;
    let secs = match parts[..] {
        [m, s] if s < 60 => m.checked_mul(60)?.checked_add(s)?,
        [h, m, s] if m < 60 && s < 60 => h.checked_mul(3600)?.checked_add(m * 60 + s)?,
        _ => return None,
    };
    Some(Duration::from_secs(secs))
}

/// What is left of countdown `text` seen at `at`, as `m:ss` or `h:mm:ss`; `None` once it has run out.
pub fn remaining(text: &str, at: Instant, now: Instant) -> Option<String> {
    let left = countdown(text)?.saturating_sub(now.saturating_duration_since(at));
    // Rounded up, so the display reaches 0:00 only when the time is up.
    let s = left.as_secs() + u64::from(left.subsec_nanos() > 0);
    (s > 0).then(|| crate::clock::hms(s))
}

/// Points per player through round `through`: a win 1, a draw half, a rest or a bye a win, as /td counts W.
fn points(pairings: &[Pairing], through: u32) -> BTreeMap<&str, f64> {
    let mut points = BTreeMap::new();
    for p in pairings.iter().filter(|p| p.round <= through) {
        let (black, white) = match (&p.result, &p.white) {
            (Outcome::Bye, _) => (1., 0.),
            (Outcome::Margin { black, .. }, _) => match black.partial_cmp(&0.) {
                Some(std::cmp::Ordering::Greater) => (1., 0.),
                Some(std::cmp::Ordering::Less) => (0., 1.),
                _ => (0.5, 0.5),
            },
            _ => (0., 0.),
        };
        *points.entry(p.black.as_str()).or_default() += black;
        if let Some(w) = &p.white {
            *points.entry(w.as_str()).or_default() += white;
        }
    }
    points.retain(|p, _| !p.starts_with('('));
    points
}

/// Competition places by points: ties share a place, the next skips (1, 1, 3).
fn places<'a>(points: &BTreeMap<&'a str, f64>) -> BTreeMap<&'a str, usize> {
    let mut sorted: Vec<f64> = points.values().copied().collect();
    sorted.sort_by(|a, b| b.total_cmp(a));
    points.iter().map(|(p, mine)| (*p, 1 + sorted.partition_point(|other| other > mine))).collect()
}

/// The last round with every round up to it decided; 0 before the first.
pub fn decided(pairings: &[Pairing]) -> u32 {
    let undecided =
        pairings.iter().filter(|p| matches!(p.result, Outcome::Playing | Outcome::Unstarted)).map(|p| p.round).min();
    let last = pairings.iter().map(|p| p.round).max().unwrap_or(0);
    undecided.map_or(last, |r| r.saturating_sub(1))
}

/// How many places each player rose (negative: fell) in the last decided round; empty before two.
pub fn movement(pairings: &[Pairing]) -> BTreeMap<String, i64> {
    let d = decided(pairings);
    if d < 2 {
        return BTreeMap::new();
    }
    let (before, after) = (places(&points(pairings, d - 1)), places(&points(pairings, d)));
    after.into_iter().map(|(p, now)| (p.to_string(), before.get(p).map_or(0, |&b| b as i64 - now as i64))).collect()
}

/// A block whose head starts `/td: `, with the `|` of its further lines removed.
pub fn parse(msg: Vec<String>) -> TdMsg {
    let head = msg[0].strip_prefix("/td: ").unwrap_or("");
    let body = &msg[1..];
    let parsed = if let Some(err) = head.strip_prefix("ERR ") {
        error(err)
    } else if let Some(n) = head.strip_prefix("tourneys: ") {
        n.trim().parse::<usize>().ok().map(|_| TdMsg::Tourneys(body.iter().filter_map(|l| summary(l)).collect()))
    } else if head.starts_with("finger: ") {
        info(body)
    } else if head.starts_with("st: ") || head.starts_with("sr: ") {
        tournament_id(head).map(|id| TdMsg::Schedule { id, pairings: body.iter().filter_map(|l| pairing(l)).collect() })
    } else {
        head.strip_prefix("rankings: tournament ").map(|id| TdMsg::Rankings {
            id: id.trim().into(),
            standings: body.iter().filter_map(|l| standing(l)).collect(),
        })
    };
    parsed.unwrap_or(TdMsg::Unknown(msg))
}

/// `2: st: can't issue command before tournament starts`.
fn error(err: &str) -> Option<TdMsg> {
    let (code, rest) = err.split_once(": ")?;
    let (command, text) = rest.split_once(": ")?;
    Some(TdMsg::Error { code: code.parse().ok()?, command: command.into(), text: text.into() })
}

/// The id ending `... tournament <id>:`.
fn tournament_id(head: &str) -> Option<String> {
    let id = head.trim_end().strip_suffix(':')?.rsplit(' ').next()?;
    (!id.is_empty()).then(|| id.into())
}

/// `1 root /os 2 0/  2 1:48 1:00//0:30 8 [Winner: nra]`; the round column pads its total, so `2/2` may be one token or two.
fn summary(line: &str) -> Option<Summary> {
    let t: Vec<&str> = line.split_whitespace().collect();
    let [id, _director, _service, players, round, rest @ ..] = t.as_slice() else { return None };
    let players = players.parse().ok()?;
    let (round, rest) = match round.strip_suffix('/') {
        Some(played) => (format!("{played}/{}", rest.first()?), rest.get(1..)?),
        None => (round.to_string(), rest),
    };
    let (played, rounds) = round.split_once('/')?;
    let [start, clock, game_type, tail @ ..] = rest else { return None };
    let start = match *start {
        "now" => Start::Now,
        "done" => Start::Done,
        s => Start::In(s.into()),
    };
    if !matches!(tail, [] | ["Winner:", _]) {
        return None;
    }
    Some(Summary {
        id: id.to_string(),
        players,
        played: played.parse().ok()?,
        rounds: round_number(rounds)?,
        start,
        clock: clock.to_string(),
        game_type: game_type.to_string(),
    })
}

/// `key : value` lines, then a status line; the `t <login> finger: ` line /td puts first is skipped.
fn info(body: &[String]) -> Option<TdMsg> {
    let mut fields = BTreeMap::new();
    let mut status = Vec::new();
    for line in body {
        match line.split_once(" : ") {
            Some((k, v)) => {
                fields.insert(k.trim(), v.trim());
            }
            None if line.trim_end().ends_with("finger:") => {}
            None if !line.trim().is_empty() => status.push(line.trim()),
            None => {}
        }
    }
    let (played, rounds) = fields.get("rounds")?.split_once('/')?;
    let players = fields.get("players")?.split_whitespace().skip(1).filter(|n| !n.starts_with('('));
    Some(TdMsg::Info(Info {
        id: fields.get("id")?.to_string(),
        played: played.parse().ok()?,
        rounds: round_number(rounds)?,
        style: match *fields.get("style")? {
            "rrobin" => Style::RoundRobin,
            "swiss" => Style::Swiss,
            _ => return None,
        },
        clock: fields.get("clock")?.to_string(),
        game_type: fields.get("type")?.to_string(),
        players: players.map(str::to_string).collect(),
        status: status.join(", "),
    }))
}

/// `1.1 nrb nra +0.0` or `1.2 nra rests`; `round N:` headers carry nothing the board number lacks.
fn pairing(line: &str) -> Option<Pairing> {
    let t: Vec<&str> = line.split_whitespace().collect();
    let (rb, black, white, result) = match t.as_slice() {
        [rb, black, "rests"] => (*rb, *black, None, Outcome::Bye),
        [rb, black, white, result] => (*rb, *black, Some(white.to_string()), outcome(result)?),
        _ => return None,
    };
    let (round, board) = rb.split_once('.')?;
    Some(Pairing { round: round_number(round)?, board: board.parse().ok()?, black: black.into(), white, result })
}

fn round_number(text: &str) -> Option<u32> {
    text.parse().ok().filter(|&n| n <= MAX_ROUNDS)
}

fn outcome(token: &str) -> Option<Outcome> {
    Some(match token {
        "?" => Outcome::Playing,
        "unstarted" => Outcome::Unstarted,
        "bye" => Outcome::Bye,
        _ => {
            let (margin, ending) = match token.split_once(':') {
                None => (token, Ending::Normal),
                Some((m, "r")) => (m, Ending::Resigned),
                Some((m, "t")) => (m, Ending::Timeout),
                Some(_) => return None,
            };
            Outcome::Margin { black: margin.parse().ok().filter(|m: &f64| m.is_finite())?, ending }
        }
    })
}

/// `1.5 ( 1  1  0) {   9.00 }      nra [0.4102]`.
fn standing(line: &str) -> Option<Standing> {
    let line = line.replace(['(', ')', '{', '}', '[', ']'], " ");
    let [points, wins, draws, losses, discs, login, tiebreak] = line.split_whitespace().collect::<Vec<_>>()[..] else {
        return None;
    };
    Some(Standing {
        points: points.parse().ok()?,
        wins: wins.parse().ok()?,
        draws: draws.parse().ok()?,
        losses: losses.parse().ok()?,
        discs: discs.parse().ok()?,
        login: login.into(),
        tiebreak: tiebreak.parse().ok()?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::{self, ServerMsg};

    fn lines(l: &[&str]) -> Vec<String> {
        l.iter().map(|s| s.to_string()).collect()
    }

    /// fixtures/live-tourney.txt through the same path as the app: the /td replies and the broadcasts, in order.
    fn replay() -> (Vec<TdMsg>, Vec<Vec<String>>) {
        let mut a = crate::net::Assembler::default();
        let (mut replies, mut news) = (Vec::new(), Vec::new());
        for msg in include_str!("../fixtures/live-tourney.txt").lines().filter_map(|l| a.feed(l.to_string()).unwrap()) {
            match proto::parse(msg) {
                ServerMsg::Td(m) => replies.push(m),
                ServerMsg::TdNews(l) => news.push(l),
                _ => {}
            }
        }
        (replies, news)
    }

    fn margin(black: f64) -> Outcome {
        Outcome::Margin { black, ending: Ending::Normal }
    }

    fn pair(round: u32, black: &str, white: &str, result: Outcome) -> Pairing {
        Pairing { round, board: 1, black: black.into(), white: Some(white.into()), result }
    }

    #[test]
    fn the_live_fixture_fills_tourneys_info_schedules_and_rankings() {
        let (r, news) = replay();
        assert_eq!(r.len(), 30);
        let TdMsg::Tourneys(list) = &r[0] else { panic!("{:?}", r[0]) };
        assert_eq!(
            list[0],
            Summary {
                id: "1".into(),
                players: 2,
                played: 0,
                rounds: 2,
                start: Start::In("1:48".into()),
                clock: "1:00//0:30".into(),
                game_type: "8".into(),
            }
        );
        let TdMsg::Tourneys(done) = &r[23] else { panic!("{:?}", r[23]) };
        assert_eq!(
            done.iter().map(|s| (s.played, &s.start, s.players)).collect::<Vec<_>>(),
            [(2, &Start::Done, 2), (2, &Start::Done, 3)],
            "the swiss count includes its (bye)"
        );

        let TdMsg::Info(f1) = &r[2] else { panic!("{:?}", r[2]) };
        assert_eq!((f1.id.as_str(), f1.style, f1.played, f1.rounds), ("1", Style::RoundRobin, 0, 2));
        assert_eq!((f1.players.clone(), f1.status.as_str()), (lines(&["nra", "nrb"]), "round 1 begins in  1:46"));
        assert!(!f1.started());
        let TdMsg::Info(f2) = &r[10] else { panic!("{:?}", r[10]) };
        assert_eq!(
            (f2.style, f2.players.clone()),
            (Style::Swiss, lines(&["nrb", "nra"])),
            "the (bye) filler is not a player"
        );
        assert_eq!(f2.status, "round 1 is being played");
        assert!(f2.started());
        let TdMsg::Info(f1_end) = &r[29] else { panic!("{:?}", r[29]) };
        assert_eq!((f1_end.played, f1_end.status.as_str()), (2, "tournament has finished"));

        assert_eq!(
            r[7],
            TdMsg::Schedule { id: "1".into(), pairings: vec![pair(1, "nrb", "nra", Outcome::Playing)] },
            "sr"
        );
        assert_eq!(
            r[12],
            TdMsg::Schedule {
                id: "1".into(),
                pairings: vec![pair(1, "nrb", "nra", margin(0.)), pair(2, "nra", "nrb", Outcome::Unstarted)]
            }
        );
        assert_eq!(
            r[14],
            TdMsg::Schedule { id: "2".into(), pairings: vec![pair(1, "nrb", "nra", margin(0.))] },
            "swiss lists only paired rounds"
        );
        assert_eq!(
            r[20],
            TdMsg::Schedule {
                id: "1".into(),
                pairings: vec![pair(1, "nrb", "nra", margin(0.)), pair(2, "nra", "nrb", margin(18.))]
            }
        );

        assert_eq!(
            r[19],
            TdMsg::Rankings {
                id: "1".into(),
                standings: vec![
                    Standing {
                        points: 1.5,
                        wins: 1,
                        draws: 1,
                        losses: 0,
                        discs: 9.,
                        login: "nra".into(),
                        tiebreak: 0.4102
                    },
                    Standing {
                        points: 0.5,
                        wins: 0,
                        draws: 1,
                        losses: 1,
                        discs: -9.,
                        login: "nrb".into(),
                        tiebreak: 0.5898
                    },
                ]
            }
        );

        let err =
            |code: u32, command: &str, text: &str| TdMsg::Error { code, command: command.into(), text: text.into() };
        assert_eq!(r[3], err(2, "st", "can't issue command before tournament starts"));
        assert_eq!(r[5], err(16, "rankings", "tournament id does not exist"));
        assert_eq!(r[28], err(18, "sr", "round does not exist"));
        assert!(matches!(&r[27], TdMsg::Unknown(l) if l[0].starts_with("/td: sp:")));

        assert_eq!(news.len(), 24, ".chat and .tourney blocks from /td");
        assert_eq!(news[0][1], "---------------- /td --------------", "the | prefix is gone");
        assert!(news.iter().any(|n| n == &lines(&[".chat /td: Congratulations to nra, winner of tournament 1"])));
    }

    #[test]
    fn replies_fill_the_state_by_id_and_sr_replaces_only_its_round() {
        let (replies, _) = replay();
        let mut t = Tourneys::default();
        for m in replies.iter().take(18).cloned() {
            t.apply(m, Instant::now());
        }
        assert!(t.started("1") && t.started("2"));
        assert_eq!(t.schedules["1"], [pair(1, "nrb", "nra", margin(0.)), pair(2, "nra", "nrb", Outcome::Playing)]);
        assert_eq!(
            t.schedules["2"],
            [pair(1, "nrb", "nra", margin(0.)), pair(2, "nra", "nrb", Outcome::Playing)],
            "sr adds a round st had not listed"
        );
        for m in replies.into_iter().skip(18) {
            t.apply(m, Instant::now());
        }
        assert_eq!(t.rows().len(), 2);
        assert_eq!(t.standings["2"][0].login, "nrb");
        assert_eq!(t.schedules["1"][1].result, margin(18.));
        assert!(!Tourneys::default().started("1"));
    }

    fn board(round: u32, board: u32, black: &str, white: Option<&str>, result: Outcome) -> Pairing {
        Pairing { round, board, black: black.into(), white: white.map(Into::into), result }
    }

    #[test]
    fn places_and_movement_come_from_the_schedule() {
        let (replies, _) = replay();
        let mut t = Tourneys::default();
        for m in replies {
            t.apply(m, Instant::now());
        }
        let final1 = &t.schedules["1"];
        assert_eq!(decided(final1), 2);
        assert_eq!(places(&points(final1, 1)), BTreeMap::from([("nra", 1), ("nrb", 1)]), "a draw shares first place");
        assert_eq!(points(final1, 2), BTreeMap::from([("nra", 1.5), ("nrb", 0.5)]), "the rankings' points");
        assert_eq!(movement(final1), BTreeMap::from([("nra".into(), 0), ("nrb".into(), -1)]));
        assert!(movement(&final1[..1]).is_empty(), "no arrows before two decided rounds");
        assert_eq!(decided(&[final1[0].clone(), pair(2, "nra", "nrb", Outcome::Playing)]), 1);
        assert_eq!(decided(&[]), 0);
        assert_eq!(decided(&[pair(0, "nra", "nrb", Outcome::Playing)]), 0, "a round 0 from /td does not underflow");

        let three = [
            board(1, 1, "a", Some("b"), margin(10.)),
            board(1, 2, "c", None, Outcome::Bye),
            board(2, 1, "c", Some("a"), margin(4.)),
            board(2, 2, "b", Some("(bye)"), Outcome::Bye),
            board(3, 1, "b", Some("c"), margin(-2.)),
            board(3, 2, "a", Some("(bye)"), Outcome::Bye),
        ];
        assert_eq!(
            points(&three, 1),
            BTreeMap::from([("a", 1.), ("b", 0.), ("c", 1.)]),
            "both rest shapes count as a win; the filler is no player"
        );
        assert_eq!(places(&points(&three, 2)), BTreeMap::from([("a", 2), ("b", 2), ("c", 1)]));
        assert_eq!(
            movement(&three[..4]),
            BTreeMap::from([("a".into(), -1), ("b".into(), 1), ("c".into(), 0)]),
            "a, b, c from 1, 3, 1 to 2, 2, 1"
        );
        assert_eq!(places(&points(&three, 3)), BTreeMap::from([("a", 2), ("b", 3), ("c", 1)]));
    }

    #[test]
    fn a_rows_progress_counts_down_its_round_and_goes_once_finished() {
        let at = Instant::now();
        let later = at + Duration::from_secs(30);
        let summary = |played, start| Summary {
            id: "1".into(),
            players: 4,
            played,
            rounds: 3,
            start,
            clock: String::new(),
            game_type: String::new(),
        };
        let progress = |played, start: Start| {
            let s = summary(played, start);
            let mut t = Tourneys::default();
            t.apply(TdMsg::Tourneys(vec![s.clone()]), at);
            t.progress(&s, later)
        };
        assert_eq!(progress(0, Start::In("1:00".into())).unwrap(), "starts in 0:30");
        assert_eq!(progress(0, Start::In("0:10".into())).unwrap(), "starting...", "not \"starts starting...\"");
        assert_eq!(progress(1, Start::In("0:10".into())).unwrap(), "round 2 of 3 starting...");
        assert_eq!(progress(3, Start::Now).unwrap(), "round 3 of 3", "the last round is not overrun");
        assert_eq!(progress(3, Start::Done), None);
    }

    #[test]
    fn countdowns_run_down_from_when_the_reply_arrived() {
        assert_eq!(countdown(" 1:46"), Some(Duration::from_secs(106)));
        assert_eq!(countdown("1:02:03"), Some(Duration::from_secs(3723)));
        assert_eq!((countdown("1:60"), countdown("now"), countdown("1"), countdown("")), (None, None, None, None));
        assert_eq!(countdown(&format!("{}:00", u64::MAX)), None, "an overflowing countdown is not one");
        let at = Instant::now();
        let after = |s: u64, ms: u64| at + Duration::from_secs(s) + Duration::from_millis(ms);
        assert_eq!(remaining("1:46", at, at).as_deref(), Some("1:46"));
        assert_eq!(remaining("1:46", at, after(46, 500)).as_deref(), Some("1:00"), "a part second still shows");
        assert_eq!(remaining("1:46", at, after(105, 0)).as_deref(), Some("0:01"));
        assert_eq!(remaining("1:46", at, after(106, 0)), None);
        assert_eq!(remaining("1:46", at, after(500, 0)), None);
        assert_eq!(remaining("1:00:00", at, after(1, 0)).as_deref(), Some("59:59"));
        assert_eq!(remaining("2:00:00", at, at).as_deref(), Some("2:00:00"));

        let (replies, _) = replay();
        let mut t = Tourneys::default();
        t.apply(replies[2].clone(), at);
        assert_eq!(t.status("1", after(6, 0)).as_deref(), Some("round 1 begins in 1:40"));
        assert_eq!(t.status("1", after(200, 0)).as_deref(), Some("round 1 starting..."));
        t.apply(replies[10].clone(), at);
        assert_eq!(t.status("2", after(200, 0)).as_deref(), Some("round 1 is being played"));
        assert_eq!(t.status("9", at), None);
    }

    #[test]
    fn the_list_and_finger_count_down_to_the_newest_reply() {
        let at = Instant::now();
        let (replies, _) = replay();
        let mut t = Tourneys::default();
        t.apply(replies[2].clone(), at);
        // A list newer than finger's 1:46 says otherwise; both go by the newer.
        let row = Summary {
            id: "1".into(),
            players: 2,
            played: 0,
            rounds: 1,
            start: Start::In("1:40".into()),
            clock: String::new(),
            game_type: String::new(),
        };
        t.apply(TdMsg::Tourneys(vec![row.clone()]), at + Duration::from_millis(1500));
        let now = at + Duration::from_secs(10);
        assert_eq!(t.progress(&row, now).as_deref(), Some("starts in 1:32"));
        assert_eq!(t.status("1", now).as_deref(), Some("round 1 begins in 1:32"));
    }

    #[test]
    fn a_tournament_finishes_by_the_list_or_by_finger() {
        let (replies, _) = replay();
        let mut t = Tourneys::default();
        for m in replies.iter().take(23).cloned() {
            t.apply(m, Instant::now());
        }
        assert!(!t.finished("1"), "rankings alone do not end it");
        t.apply(replies[23].clone(), Instant::now());
        assert!(t.finished("1") && t.finished("2"));
        let mut t = Tourneys::default();
        for m in [&replies[18], &replies[19], &replies[29]] {
            t.apply(m.clone(), Instant::now());
        }
        assert!(t.finished("1"), "finger's end");
        assert_eq!(t.players("1"), ["nra", "nrb"]);
        t.apply(replies[10].clone(), Instant::now());
        assert_eq!(t.players("2"), ["nrb", "nra"], "finger's order before rankings, without the filler");
    }

    #[test]
    fn the_player_count_leaves_out_the_filler_once_finger_names_it() {
        let TdMsg::Tourneys(rows) = parse(lines(&[
            "/td: tourneys: 1",
            "   id director  game #pl   round start             clock       type",
            "    2     root   /os   3   1/  2  now         1:00//0:30          8",
        ])) else {
            panic!()
        };
        let mut t = Tourneys::default();
        t.apply(TdMsg::Tourneys(rows.clone()), Instant::now());
        assert_eq!(t.player_count(&rows[0]), 3, "the list alone counts the filler");
        t.apply(
            parse(lines(&[
                "/td: finger: 2",
                "t me finger: ",
                "id        : 2",
                "joining   : closed",
                "director  : root",
                "rounds    : 1/2",
                "style     : rrobin",
                "clock     : 1:00//0:30",
                "type      : 8",
                "players   : 3 nrb nra (bye) ",
                "round 2 is being played",
            ])),
            Instant::now(),
        );
        assert_eq!(t.player_count(&rows[0]), 2);
    }

    #[test]
    fn the_crosstable_shows_each_game_from_the_row_players_side() {
        let (replies, _) = replay();
        let mut t = Tourneys::default();
        for m in replies {
            t.apply(m, Instant::now());
        }
        let players = t.players("1");
        assert_eq!(
            crosstable(&players, &t.schedules["1"]),
            [
                vec![vec![], vec![CellGame::Margin(0.), CellGame::Margin(18.)]],
                vec![vec![CellGame::Margin(0.), CellGame::Margin(-18.)], vec![]],
            ],
            "nra was white in round 1 and black in round 2"
        );
        let CellGame::Margin(flipped) = crosstable(&players, &t.schedules["1"])[0][1][0] else { unreachable!() };
        assert!(flipped.is_sign_positive(), "a draw shows +0 from either side");

        let three = [
            board(1, 1, "a", Some("b"), margin(-6.)),
            board(1, 2, "c", Some("(bye)"), Outcome::Bye),
            board(2, 1, "c", Some("a"), Outcome::Playing),
            board(2, 2, "b", None, Outcome::Bye),
            board(3, 1, "b", Some("c"), Outcome::Unstarted),
            board(3, 2, "x", Some("a"), margin(2.)),
        ];
        let players: Vec<String> = ["c", "a", "b"].map(String::from).into();
        use CellGame::*;
        assert_eq!(
            crosstable(&players, &three),
            [
                vec![vec![], vec![Playing], vec![Unstarted]],
                vec![vec![Playing], vec![], vec![Margin(-6.)]],
                vec![vec![Unstarted], vec![Margin(6.)], vec![]],
            ],
            "rests, the filler and unlisted players have no cell"
        );
    }

    #[test]
    fn a_count_every_decided_meeting_shares_is_left_out() {
        use CellGame::*;
        let table = |cells: Vec<Vec<CellGame>>| vec![cells];
        assert_eq!(shared_decided(&table(vec![vec![Margin(2.), Margin(-1.)], vec![Margin(0.), Margin(4.)]])), Some(2));
        assert_eq!(
            shared_decided(&table(vec![vec![Margin(2.), Playing], vec![], vec![Margin(1.)]])),
            Some(1),
            "a game still playing and an empty cell add no count"
        );
        assert_eq!(shared_decided(&table(vec![vec![Margin(2.), Margin(-1.)], vec![Margin(0.)]])), None);
        assert_eq!(shared_decided(&table(vec![vec![Unstarted]])), None);
    }

    #[test]
    fn a_swiss_row_holds_each_round_from_its_players_side() {
        let players: Vec<String> = ["a", "b", "c"].map(Into::into).into();
        let margin = |black| Outcome::Margin { black, ending: Ending::Normal };
        let schedule = [
            board(1, 1, "a", Some("b"), margin(4.)),
            board(1, 2, "c", None, Outcome::Bye),
            board(2, 1, "c", Some("a"), Outcome::Playing),
            board(2, 2, "b", Some("(bye)"), Outcome::Bye),
        ];
        let met = |opponent, game| Some(RoundCell::Met { opponent, game });
        assert_eq!(
            rounds_table(&players, &schedule, 3),
            [
                vec![met(1, CellGame::Margin(4.)), met(2, CellGame::Playing), None],
                vec![met(0, CellGame::Margin(-4.)), Some(RoundCell::Bye), None],
                vec![Some(RoundCell::Bye), met(0, CellGame::Playing), None],
            ]
        );
    }

    #[test]
    fn rows_missing_from_the_fixture_parse_or_are_dropped() {
        let st = |rows: &[&str]| match parse(lines(
            &[&["/td: st: schedule for tournament 3:", "", "round   1:"], rows].concat(),
        )) {
            TdMsg::Schedule { id, pairings } => {
                assert_eq!(id, "3");
                pairings
            }
            m => panic!("{m:?}"),
        };
        let p = st(&[
            "  1.1       nra      nrb  -3.0:r",
            "  1.2       ant      bob  +0.0:t",
            "  1.3       cat    (bye)     bye",
            "  1.4       dan rests",
            "garbage",
        ]);
        assert_eq!(
            p.iter().map(|p| (p.board, p.white.as_deref(), p.result.clone())).collect::<Vec<_>>(),
            [
                (1, Some("nrb"), Outcome::Margin { black: -3., ending: Ending::Resigned }),
                (2, Some("bob"), Outcome::Margin { black: 0., ending: Ending::Timeout }),
                (3, Some("(bye)"), Outcome::Bye),
                (4, None, Outcome::Bye),
            ]
        );
        assert!(
            st(&["  1.1  a  b  +1.0:x", "  1.2  a  b  NaN"]).is_empty(),
            "unknown endings and non-finite margins are dropped"
        );
        match parse(lines(&[
            "/td: tourneys: 1",
            "   id director  game #pl   round start             clock       type",
            "   12     root   /os  10  10/ 10  now  5:00//2:00  8r   Winner: x",
        ])) {
            TdMsg::Tourneys(l) => {
                assert_eq!((l[0].id.as_str(), l[0].played, l[0].rounds, &l[0].start), ("12", 10, 10, &Start::Now))
            }
            m => panic!("{m:?}"),
        }
        match parse(lines(&["/td: tourneys: 1", "  4 root /os 2 3/100 0:05 1:00//0:30 8"])) {
            TdMsg::Tourneys(l) => {
                assert_eq!((l[0].played, l[0].rounds, &l[0].start), (3, 100, &Start::In("0:05".into())));
                let mut t = Tourneys::default();
                t.apply(TdMsg::Tourneys(l), Instant::now());
                assert!(t.started("4"), "a countdown after a played round is a break, not the start");
            }
            m => panic!("{m:?}"),
        }
        let rows = [
            "/td: tourneys: 3",
            "  2 root /os 2 0/2 1:00 1:00//0:30 8",
            "  10 root /os 2 0/2 1:00 1:00//0:30 8",
            "  9 root /os 2 0/2 1:00 1:00//0:30 8",
        ];
        let mut t = Tourneys::default();
        t.apply(parse(lines(&rows)), Instant::now());
        assert_eq!(t.rows().iter().map(|s| s.id.as_str()).collect::<Vec<_>>(), ["10", "9", "2"], "newest first");
        assert!(matches!(parse(lines(&["/td: frobnicate"])), TdMsg::Unknown(_)));
        assert_eq!(tell("finger 1"), "tell /td finger 1");
    }

    #[test]
    fn absurd_round_numbers_are_dropped() {
        assert!(pairing("1000.1 nrb nra +0.0").is_some());
        assert!(pairing("1001.1 nrb nra +0.0").is_none());
        assert!(summary("1 root /os 2 0/4000000000 now 1:48 8").is_none());
    }
}
