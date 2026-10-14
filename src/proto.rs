use crate::chat::{ChatKey, ChatLine};
use crate::ggf::GameRecord;
use crate::othello::MoveToken;

#[derive(Debug)]
pub enum ServerMsg {
    /// A `: + user` / `: - user` notice block; `os` is set when it announces `/os`.
    Presence {
        os: Option<bool>,
    },
    /// A help file from the server (`: help news`) or from `/os` (`/os: help help`).
    Help(HelpText),
    /// A channel line `.chat bob: hi` or a private tell `bob: hi` (Central EXE_Client.C `tell_send`,
    /// VC_SEQ.C `text`). Neither comes back to its sender, except a tell to oneself.
    Chat {
        key: ChatKey,
        line: ChatLine,
    },
    /// The server's own `finger <login> ..`: one profile per login, and the logins it does not know.
    Finger {
        profiles: Vec<Profile>,
        missing: Vec<String>,
        lines: Vec<String>,
    },
    Os(OsMsg),
    /// `: ERR user '/os' not found.`: a tell to /os failed, so /os is not on.
    OsAbsent(Vec<String>),
    /// A reply from the tournament director.
    Td(crate::td::TdMsg),
    /// A `/td` broadcast on a channel (`.chat /td: ...`, `.tourney /td: ...`).
    TdNews(Vec<String>),
    Other(Vec<String>),
}

/// What `finger` shows anyone about a login (Central VAR_Client.C `print`).
#[derive(Clone, PartialEq, Debug, Default)]
pub struct Profile {
    pub login: String,
    pub name: String,
    /// Set by the user with `info <text>`; a multi-line info repeats the `info   :` key.
    pub info: Vec<String>,
    pub seen: Seen,
    pub channels: String,
}

/// Whether a login is on, from `finger`'s `since`/`idle` (on) or `last` (off) lines.
#[derive(Clone, PartialEq, Debug, Default)]
pub enum Seen {
    #[default]
    Unknown,
    /// `idle` is `00:00:22, on line : 07:50:10`.
    On {
        since: String,
        idle: Option<String>,
    },
    Off {
        last: String,
    },
}

/// `: finger` blocks of `key : value` lines, one per login, or `: finger ERR not found: a b`; the
/// ack of the command comes last in the same block.
fn parse_profiles(msg: Vec<String>) -> ServerMsg {
    let mut profiles: Vec<Profile> = Vec::new();
    let mut missing = Vec::new();
    for line in &msg {
        if line == ": finger" {
            profiles.push(Profile::default());
        } else if let Some(names) = line.strip_prefix(": finger ERR not found:") {
            missing.extend(names.split_whitespace().map(str::to_string));
        } else if let (Some(p), Some((key, value))) = (profiles.last_mut(), line.split_once(':')) {
            let value = value.trim().to_string();
            match key.trim().split('(').next().unwrap_or("") {
                "login" => p.login = value,
                "name" => p.name = value,
                "info" => p.info.push(value),
                "since" => p.seen = Seen::On { since: value, idle: None },
                "idle" => {
                    if let Seen::On { idle, .. } = &mut p.seen {
                        *idle = Some(value);
                    }
                }
                "last" if !matches!(p.seen, Seen::On { .. }) => p.seen = Seen::Off { last: value },
                "channs" => p.channels = value,
                _ => {}
            }
        }
    }
    for p in &mut profiles {
        if p.info.iter().all(String::is_empty) {
            p.info.clear();
        }
    }
    ServerMsg::Finger { profiles, missing, lines: msg }
}

#[derive(Clone, PartialEq, Debug)]
pub struct HelpText {
    /// The header without the `: ` prefix, e.g. `help news` or `/os: help help`.
    pub title: String,
    pub lines: Vec<String>,
}

#[derive(Debug)]
pub enum OsMsg {
    MatchList(Vec<MatchEntry>),
    MatchAdd(MatchEntry),
    /// `finished` is `None` when the line is too short to name the players.
    MatchRemove {
        id: String,
        finished: Option<Finished>,
    },
    Join {
        id: String,
        record: Box<GameRecord>,
    },
    Update {
        id: String,
        token: MoveToken,
    },
    End {
        id: String,
        outcome: Outcome,
    },
    WatchAdd {
        name: String,
        id: String,
    },
    WatchRemove {
        name: String,
        id: String,
    },
    GameText {
        id: String,
        lines: Vec<String>,
    },
    /// A kibitz line for a game. The sender never gets its own back (VC_SEQ.C `text`).
    Kibitz {
        id: String,
        line: ChatLine,
    },
    /// A `who` list with the message as sent, header included, for the log.
    Who {
        users: Vec<User>,
        lines: Vec<String>,
    },
    /// `watch + ERR not found:`: these matches do not exist, so the server sends nothing for them.
    WatchMissing {
        ids: Vec<String>,
    },
    /// Match settings from `/os finger`, and logins it does not know.
    Finger {
        settings: Vec<MatchSettings>,
        missing: Vec<String>,
    },
    /// An archived match from `look`: one record, or two for a synchro match. The reply does not say
    /// which match it is.
    Look(Vec<GameRecord>),
    /// `look` found nothing under this key.
    LookMissing {
        id: String,
    },
    /// The rated matches a login finished, oldest first.
    History {
        login: String,
        games: Vec<Archived>,
    },
    /// The adjourned matches a login can continue.
    Stored {
        login: String,
        games: Vec<Archived>,
    },
    /// A `top` ranking, best first, with the message as sent. The reply does not say which rating type it is.
    Top {
        ranks: Vec<Rank>,
        lines: Vec<String>,
    },
    /// A recognized message whose content failed to parse.
    Bad {
        reply: Option<ReplyKind>,
        lines: Vec<String>,
        error: String,
    },
    Unknown(Vec<String>),
}

#[derive(Clone, Copy, Debug)]
pub enum ReplyKind {
    Look,
    Who,
    Top,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum UserStatus {
    /// `+`
    Open,
    /// `-`
    Busy,
    /// `x`
    Ghost,
}

#[derive(Clone, PartialEq, Debug)]
pub struct User {
    pub name: String,
    pub status: UserStatus,
    pub rating: String,
    /// The match the user is in, the `.N` that ends a live `who` row.
    pub playing: Option<String>,
}

/// Public match-request settings from the Othello service's `finger` reply.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct MatchSettings {
    pub login: String,
    pub open: Option<usize>,
    pub playing: Option<usize>,
    pub rated: Option<bool>,
    pub trust: Option<bool>,
    pub accept: Option<String>,
    pub decline: Option<String>,
}

/// The rating pools a viewer lists: plain 8x8 and 8x8 from random starts (`8r`, which synchro `s8r`
/// games are rated in).
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub enum RatingType {
    #[default]
    Eight,
    EightRandom,
}

/// How many players the GUI asks `top` for; only replies to exactly this count fill its rankings cache.
pub const GUI_TOP_COUNT: &str = "24";

/// Whether a list reply is expected and which GUI cache it can replace; filtered/unsupported replies use `Some(None)`.
/// Invalid requests return only ERR and must not reserve a list reply (EXE_Service.C who/top).
pub fn list_query_cache_type(command: &str, args: &[&str]) -> Option<Option<RatingType>> {
    let valid = match command {
        "who" => matches!(args.len(), 1 | 3) || matches!(args.len(), 2 | 4) && args[1] == "a",
        "top" => (1..=3).contains(&args.len()) && args[1..].iter().all(|s| s.bytes().all(|b| b.is_ascii_digit())),
        _ => false,
    };
    if !valid {
        return None;
    }
    // MatchType::normalize_key accepts a board size followed by R/A in either case and order.
    let key = args[0].to_ascii_lowercase();
    let number = key.trim_end_matches(['r', 'a']);
    let board = number.parse::<u8>().ok()?;
    if !matches!(board, 4 | 6 | 8 | 10 | 12 | 14 | 88) {
        return None;
    }
    let cacheable = match (command, args) {
        // Validity admits only `a` here, which lists every user, not only those open for a match.
        ("who", [_, _]) => true,
        ("top", [_, count]) => *count == GUI_TOP_COUNT,
        _ => false,
    };
    Some((cacheable && board == 8 && !key.contains('a')).then_some(if key.contains('r') {
        RatingType::EightRandom
    } else {
        RatingType::Eight
    }))
}

impl RatingType {
    pub fn key(self) -> &'static str {
        match self {
            RatingType::Eight => "8",
            RatingType::EightRandom => "8r",
        }
    }
}

/// One `top` row (VC_Rank.C `print`, VC_Rating.C `Rating::print`).
#[derive(Clone, PartialEq, Debug)]
pub struct Rank {
    pub place: u32,
    pub name: String,
    pub rating: f64,
    pub stdev: f64,
    /// Average result per game, `AScore`.
    pub ascore: f64,
    pub win: u32,
    pub draw: u32,
    pub loss: u32,
}

impl Rank {
    /// `    1 edax     2037.8@290.2=     00:00:17+@290.2 +64.0      1      0      0`. The fields are
    /// printf-padded, so a narrow deviation leaves a space after `@` and a wide average runs into the
    /// deviation before it (`+@290.2-317.8`).
    fn parse(line: &str) -> Option<Rank> {
        let s = close_deviation(line.trim_end().trim_end_matches("<="));
        let f: Vec<&str> = s.split_whitespace().collect();
        let [place, name, rating, middle @ .., win, draw, loss] = &f[..] else { return None };
        let (rating, stdev) = rating.trim_end_matches('=').split_once('@')?;
        let tail = middle.concat();
        let tail = &tail[tail.rfind('@')? + 1..];
        let sign = tail.char_indices().skip(1).find(|(_, c)| matches!(c, '+' | '-'))?.0;
        Some(Rank {
            place: place.parse().ok()?,
            name: name.to_string(),
            rating: rating.parse::<f64>().ok().filter(|n| n.is_finite())?,
            stdev: stdev.parse::<f64>().ok().filter(|n| n.is_finite() && *n >= 0.0)?,
            ascore: tail[sign..].parse::<f64>().ok().filter(|n| n.is_finite())?,
            win: win.parse().ok()?,
            draw: draw.parse().ok()?,
            loss: loss.parse().ok()?,
        })
    }
}

/// A `history` or `stored` row: a match kept in the archive under `id`, the key `look` takes.
#[derive(Clone, PartialEq, Debug)]
pub struct Archived {
    pub id: String,
    pub date: String,
    pub player1: String,
    pub player2: String,
    /// Player 1's margin; `stored` rows have none.
    pub score: Option<f64>,
    /// The game type, with `:l` on a match adjourned because a player left.
    pub info: String,
}

impl Archived {
    /// `.67      01 Oct 2026 03:32:59 1720 prated   1720 edax      -64.0 8` (GAME_History.C `print`).
    fn history(line: &str) -> Option<Archived> {
        let f: Vec<&str> = line.split_whitespace().collect();
        let [id, date @ .., _, player1, _, player2, score, info] = &f[..] else { return None };
        let score = Some(score.parse::<f64>().ok().filter(|n| n.is_finite())?);
        Archived::new(id, date, player1, player2, score, info)
    }

    /// `.69      01 Oct 2026 03:35:41 prated   edax     8:l` (GAME_Stored.C `print`).
    fn stored(line: &str) -> Option<Archived> {
        let f: Vec<&str> = line.split_whitespace().collect();
        let [id, date @ .., player1, player2, info] = &f[..] else { return None };
        Archived::new(id, date, player1, player2, None, info)
    }

    /// The margin on `login`'s side; none for a `stored` row or a login that did not play.
    pub fn margin_for(&self, login: &str) -> Option<f64> {
        let s = self.score?;
        if self.player1 == login {
            Some(s)
        } else if self.player2 == login {
            Some(-s)
        } else {
            None
        }
    }

    fn new(id: &str, date: &[&str], player1: &str, player2: &str, score: Option<f64>, info: &str) -> Option<Archived> {
        (id.starts_with('.') && !date.is_empty()).then(|| Archived {
            id: id.into(),
            date: date.join(" "),
            player1: player1.into(),
            player2: player2.into(),
            score,
            info: info.into(),
        })
    }
}

/// `look` sends `<n> ` and then `n` GGF records back to back (Match.C `~Match`); a synchro match
/// has two, split after the first `;)`.
fn parse_look(body: &[String]) -> Result<Vec<GameRecord>, String> {
    let text: String = body.iter().map(|l| format!("{l} ")).collect();
    let (n, ggf) = text.trim_start().split_once(' ').ok_or("empty look reply")?;
    match n {
        "1" => Ok(vec![GameRecord::parse(ggf)?]),
        "2" => {
            let cut = ggf.find(";)").ok_or("look reply has no ';)'")? + 2;
            Ok(vec![GameRecord::parse(&ggf[..cut])?, GameRecord::parse(&ggf[cut..])?])
        }
        _ => Err(format!("look reply starts with '{n}', not a record count")),
    }
}

/// GGS pads a deviation under 100 (`2016.9@ 85.2`); joined, a rating is one field.
fn close_deviation(line: &str) -> String {
    let mut parts = line.split('@');
    let first = parts.next().unwrap_or_default().to_string();
    parts.fold(first, |s, part| s + "@" + part.trim_start_matches(' '))
}

/// One `who` line, `name status rating ... [.N]`; the service's own login and headers are skipped.
fn parse_user(line: &str) -> Option<User> {
    let line = close_deviation(line);
    let f: Vec<&str> = line.split_whitespace().collect();
    let [name, status, rating, rest @ ..] = &f[..] else { return None };
    let status = match *status {
        "+" => UserStatus::Open,
        "-" => UserStatus::Busy,
        "x" => UserStatus::Ghost,
        _ => return None,
    };
    let playing = rest.last().filter(|t| t.starts_with('.')).map(|t| t.to_string());
    (!name.starts_with('/')).then(|| User { name: name.to_string(), status, rating: rating.to_string(), playing })
}

/// `+` or `-` as a yes/no flag; anything else is unknown.
fn plus_minus(value: &str) -> Option<bool> {
    match value.split_whitespace().next() {
        Some("+") => Some(true),
        Some("-") => Some(false),
        _ => None,
    }
}

/// Keeps each login's settings separate, including empty and absent condition fields.
fn parse_finger(lines: &[String]) -> OsMsg {
    let mut settings: Vec<MatchSettings> = Vec::new();
    let mut missing = Vec::new();
    for line in lines {
        if let Some(names) = os_body(line).and_then(|b| b.strip_prefix(" finger ERR not found:")) {
            missing.extend(names.split_whitespace().map(str::to_string));
            continue;
        }
        let Some((key, value)) = line.split_once(':') else { continue };
        let key = key.trim().split('(').next().unwrap_or("").trim();
        let value = value.trim();
        if key == "login" {
            if let Some(login) = value.split_whitespace().next() {
                settings.push(MatchSettings { login: login.into(), ..Default::default() });
            }
            continue;
        }
        let Some(s) = settings.last_mut() else { continue };
        match key {
            "open" => s.open = value.parse().ok(),
            "play" => s.playing = Some(value.split_whitespace().count()),
            "rated" => s.rated = plus_minus(value),
            "trust" => s.trust = plus_minus(value),
            "accept" => s.accept = Some(value.into()),
            "decline" => s.decline = Some(value.into()),
            _ => {}
        }
    }
    OsMsg::Finger { settings, missing }
}

/// A kibitz line `<A|O|P> <rating> <name>: <text>` (to all, observers, players).
fn kibitz(tok: &[&str], more: &[String]) -> Option<ChatLine> {
    let ["A" | "O" | "P", _, name, text @ ..] = tok else { return None };
    let name = name.strip_suffix(':').filter(|n| !n.is_empty())?;
    Some(ChatLine { from: name.into(), lines: prepend(text.join(" "), more) })
}

/// `.chat bob: hi` or `bob: hi`. A sender name has no spaces and is not a service (`/os`) or the
/// server itself (`: ...`); the rest of the block is the message's further lines.
fn parse_chat(msg: &[String]) -> Option<(ChatKey, ChatLine)> {
    let head = &msg[0];
    let (channel, rest) = match head.split_once(' ') {
        Some((c, rest)) if c.len() > 1 && c.starts_with('.') => (Some(c), rest),
        _ => (None, head.as_str()),
    };
    let (from, text) = rest.split_once(": ")?;
    if from.is_empty() || from.contains(char::is_whitespace) || from.starts_with([':', '/', '.']) {
        return None;
    }
    let key = match channel {
        Some(c) => ChatKey::Channel(c.into()),
        None => ChatKey::User(from.into()),
    };
    Some((key, ChatLine { from: from.into(), lines: prepend(text.to_string(), &msg[1..]) }))
}

fn prepend(first: String, rest: &[String]) -> Vec<String> {
    std::iter::once(first).chain(rest.iter().cloned()).collect()
}

/// The login the Othello service speaks as; the one place its name lives.
pub const OS: &str = "/os";

/// The text after `/os:` in a line from the service.
fn os_body(l: &str) -> Option<&str> {
    l.strip_prefix(OS)?.strip_prefix(':')
}

/// A command for the Othello service. New accounts have no `t` alias, so the full `tell` is required.
pub fn tell(cmd: &str) -> String {
    format!("tell {OS} {cmd}")
}

/// Commands to send once `/os` is present (spec 1.4; no `open 1`: a viewer takes no match requests).
pub fn os_hello() -> Vec<String> {
    ["vt100 -", "client +", "notify + *", "notify +", "watch +", "match"].into_iter().map(tell).collect()
}

#[derive(Clone, PartialEq, Debug)]
pub struct MatchSummary {
    pub id: String,
    pub rating1: String,
    pub player1: String,
    pub rating2: String,
    pub player2: String,
    pub game_type: String,
    pub mode: String,
}

impl MatchSummary {
    /// `.id rating1 player1 rating2 player2 type mode`, and the fields after them.
    fn parse<'a>(f: &'a [&'a str]) -> Option<(MatchSummary, &'a [&'a str])> {
        let [id, rating1, player1, rating2, player2, game_type, mode, tail @ ..] = f else { return None };
        let summary = MatchSummary {
            id: id.to_string(),
            rating1: rating1.to_string(),
            player1: player1.to_string(),
            rating2: rating2.to_string(),
            player2: player2.to_string(),
            game_type: game_type.to_string(),
            mode: mode.to_string(),
        };
        Some((summary, tail))
    }
}

/// The score at the end of `- match`, from player 1's side as listed in the entry, not black's:
/// in a live game with edax black and pbot listed first, pbot resigning gave `-64.00`.
#[derive(Clone, PartialEq, Debug)]
pub struct MatchResult {
    pub player1: String,
    pub player2: String,
    pub score: f64,
}

impl MatchResult {
    /// A synchro match from its two boards, on `a`'s player 1 side: GGS scores it as the mean of the boards.
    pub fn synchro(a: &MatchResult, b: &MatchResult) -> MatchResult {
        let b_score = if b.player1 == a.player1 { b.score } else { -b.score };
        MatchResult { player1: a.player1.clone(), player2: a.player2.clone(), score: (a.score + b_score) / 2. }
    }

    /// `pbot -64 edax` and `edax wins by 64`.
    pub fn describe(&self) -> Vec<String> {
        let (p1, p2, s) = (&self.player1, &self.player2, self.score);
        let verdict = if s > 0. {
            format!("{p1} wins by {s}")
        } else if s < 0. {
            format!("{p2} wins by {}", -s)
        } else {
            "draw".to_string()
        };
        vec![format!("{p1} {} {p2}", crate::ui::signed(s, None)), verdict]
    }
}

/// How a match or a synchro board ended; the tail after the players in `- match` and `end`
/// (Match.C `~Match`, `resign`, `adjourn`, `abort`).
#[derive(Clone, PartialEq, Debug)]
pub enum Outcome {
    Score(MatchResult),
    Aborted,
    Left(String),
    /// An ending this viewer cannot read, kept as text.
    Other(Vec<String>),
}

/// A match the server reported as over, as `- match` lists it.
#[derive(Clone, PartialEq, Debug)]
pub struct Finished {
    pub summary: MatchSummary,
    pub outcome: Outcome,
    /// The key `look` takes. Only a rated match is archived, under a counter that differs from
    /// its match id (`- match .23 ... R -64.00  .73`).
    pub archive: Option<String>,
}

impl Outcome {
    /// `+30.00 [counter]`, `aborted`, `pbot left [counter]` or `player pbot left`.
    fn parse(player1: &str, player2: &str, tail: &[&str]) -> Option<Outcome> {
        if let Some(score) = tail.first().and_then(|s| s.parse::<f64>().ok()).filter(|n| n.is_finite()) {
            return Some(Outcome::Score(MatchResult { player1: player1.into(), player2: player2.into(), score }));
        }
        if tail.first() == Some(&"aborted") {
            return Some(Outcome::Aborted);
        }
        let left = tail.iter().position(|t| *t == "left")?;
        Some(Outcome::Left(tail.get(left.checked_sub(1)?)?.to_string()))
    }

    /// An ending known only as text; an empty text still says the match is over.
    pub fn finished(text: String) -> Outcome {
        Outcome::Other(vec![if text.is_empty() { "match finished".into() } else { text }])
    }

    pub fn describe(&self) -> Vec<String> {
        match self {
            Outcome::Score(r) => r.describe(),
            Outcome::Aborted => vec!["aborted".into()],
            Outcome::Left(name) => vec![format!("{name} left")],
            Outcome::Other(lines) => lines.clone(),
        }
    }
}

impl Finished {
    /// `.23 1402 prated 2038 edax 8 R -64.00  .73`, the text after `- match`.
    fn parse(f: &[&str]) -> Option<Finished> {
        let (summary, tail) = MatchSummary::parse(f)?;
        let outcome = Outcome::parse(&summary.player1, &summary.player2, tail)
            .unwrap_or_else(|| Outcome::finished(tail.join(" ")));
        let archive = tail.last().filter(|t| t.len() > 1 && t.starts_with('.')).map(|t| t.to_string());
        Some(Finished { summary, outcome, archive })
    }
}

/// `timeout .4 pbot (*)`: GGS writes the color as `*` (black), `O` (white), or `?` before komi is decided.
fn player_side(name: &str, color: Option<&str>) -> String {
    match color {
        Some("(*)") => format!("{name} (black)"),
        Some("(O)") => format!("{name} (white)"),
        _ => name.to_string(),
    }
}

/// A running-match entry, `.id rating1 player1 rating2 player2 type mode [count]` on a live server;
/// anything else is kept verbatim. A synchro match is listed once as `.N`, never as `.N.0`/`.N.1`.
#[derive(Clone, PartialEq, Debug)]
pub enum MatchEntry {
    Summary(MatchSummary),
    Raw(String),
}

impl MatchEntry {
    pub fn parse(entry: &str) -> MatchEntry {
        let f: Vec<&str> = entry.split_whitespace().collect();
        match MatchSummary::parse(&f) {
            Some((s, _)) if s.id.starts_with('.') => MatchEntry::Summary(s),
            _ => MatchEntry::Raw(entry.trim().to_string()),
        }
    }

    pub fn id(&self) -> &str {
        match self {
            MatchEntry::Summary(s) => &s.id,
            MatchEntry::Raw(raw) => raw.split_whitespace().next().unwrap_or(""),
        }
    }
}

/// Text after the `n`th space of `s`, or "" if there are fewer.
fn after_spaces(s: &str, n: usize) -> &str {
    s.splitn(n + 1, ' ').nth(n).unwrap_or("")
}

/// The `<command> => <expanded>` of the `: +ack:  ` line that ends a block (Central EXE_Client.C
/// `shell_execute`), in the same block as the command's ERR. A block led by anything but `:` is
/// someone's text, whose `|` continuation lines could spell an ack.
pub fn ack(msg: &[String]) -> Option<&str> {
    if !msg.first()?.starts_with(':') {
        return None;
    }
    msg.last()?.strip_prefix(": +ack:  ")
}

fn is_presence_line(l: &str) -> bool {
    let b = l.as_bytes();
    b.len() >= 4 && b[0] == b':' && matches!(b[2], b'+' | b'-') && b[3] == b' '
}

pub fn parse(msg: Vec<String>) -> ServerMsg {
    let Some(head) = msg.first() else { return ServerMsg::Other(msg) };
    if is_presence_line(head) {
        let os = msg.iter().filter(|l| is_presence_line(l)).find_map(|l| {
            let t: Vec<&str> = l.split_whitespace().collect();
            (t.len() <= 4 && t.get(2) == Some(&OS)).then(|| t[1] == "+")
        });
        return ServerMsg::Presence { os };
    }
    if head.starts_with(": help ") || os_body(head).is_some_and(|b| b.starts_with(" help ")) {
        // The server puts the ack of the `help` command in the same block.
        let lines = msg[1..].iter().filter(|l| !l.starts_with(": +ack:")).cloned().collect();
        return ServerMsg::Help(HelpText { title: head.trim_start_matches(": ").to_string(), lines });
    }
    if head == ": finger" || head.starts_with(": finger ERR") {
        return parse_profiles(msg);
    }
    if os_body(head).is_some() {
        return ServerMsg::Os(parse_os(msg));
    }
    if head.starts_with("/td:") {
        return ServerMsg::Td(crate::td::parse(msg));
    }
    if head.split_once(' ').is_some_and(|(c, rest)| c.starts_with('.') && rest.starts_with("/td:")) {
        return ServerMsg::TdNews(msg);
    }
    if head == ": ERR user '/os' not found." {
        return ServerMsg::OsAbsent(msg);
    }
    match parse_chat(&msg) {
        Some((key, line)) => ServerMsg::Chat { key, line },
        None => ServerMsg::Other(msg),
    }
}

fn parse_os(msg: Vec<String>) -> OsMsg {
    let head = msg[0].clone();
    let tok: Vec<&str> = head.split_whitespace().collect();
    let kw = tok.get(1).copied().unwrap_or("");
    let bad = |error: &str| OsMsg::Bad {
        reply: match kw {
            "look" => Some(ReplyKind::Look),
            "who" => Some(ReplyKind::Who),
            "top" => Some(ReplyKind::Top),
            _ => None,
        },
        lines: msg.clone(),
        error: error.into(),
    };
    match (kw, tok.get(2).copied(), tok.get(3).copied()) {
        ("match", _, _) => {
            OsMsg::MatchList(msg[1..].iter().filter(|l| !l.trim().is_empty()).map(|l| MatchEntry::parse(l)).collect())
        }
        ("+", Some("match"), Some(_)) => OsMsg::MatchAdd(MatchEntry::parse(after_spaces(&head, 3))),
        ("-", Some("match"), Some(id)) => OsMsg::MatchRemove { id: id.into(), finished: Finished::parse(&tok[3..]) },
        ("+", Some(name), Some("watch")) | ("-", Some(name), Some("watch")) => match tok.get(4) {
            Some(id) if kw == "+" => OsMsg::WatchAdd { name: name.into(), id: id.to_string() },
            Some(id) => OsMsg::WatchRemove { name: name.into(), id: id.to_string() },
            None => bad("watch notice without id"),
        },
        ("join", Some(id), _) => {
            let mut ggf = tok[3..].join(" ");
            for l in &msg[1..] {
                ggf.push_str(l);
                ggf.push(' ');
            }
            match GameRecord::parse(&ggf) {
                Ok(r) => OsMsg::Join { id: id.into(), record: Box::new(r) },
                Err(e) => bad(&e),
            }
        }
        ("update", Some(id), Some(mv)) => match MoveToken::parse(mv) {
            Ok(token) => OsMsg::Update { id: id.into(), token },
            Err(e) => bad(&e),
        },
        ("end", Some(id), _) => {
            let text = after_spaces(&head, 3).trim();
            let outcome = text.strip_prefix('(').and_then(|t| t.split_once(')')).and_then(|(names, tail)| {
                let (p1, p2) = names.split_once(" vs. ")?;
                Outcome::parse(p1.trim(), p2.trim(), &tail.split_whitespace().collect::<Vec<_>>())
            });
            OsMsg::End {
                id: id.into(),
                outcome: outcome.unwrap_or_else(|| Outcome::Other(prepend(text.to_string(), &msg[1..]))),
            }
        }
        ("undo" | "abort" | "score", Some(id), Some(name)) => {
            let line = match kw {
                "undo" => format!("{name} asks to undo the last move"),
                "abort" => format!("{name} asks to abort the game"),
                _ => format!("{name} offers to end the game on a score"),
            };
            OsMsg::GameText { id: id.into(), lines: vec![line] }
        }
        ("timeout" | "fatal-timeout" | "trust-violation", Some(id), Some(name)) => {
            let who = player_side(name, tok.get(4).copied());
            let line = match kw {
                "timeout" => format!("{who} ran out of time"),
                "fatal-timeout" => format!("{who} lost on time"),
                _ => format!("{who} reported less time than the server measured: {}", after_spaces(&head, 5)),
            };
            OsMsg::GameText { id: id.into(), lines: vec![line] }
        }
        ("comment" | "error", Some(id), _) => {
            OsMsg::GameText { id: id.into(), lines: prepend(after_spaces(&head, 3).to_string(), &msg[1..]) }
        }
        ("rating_update", Some(id), _) => {
            OsMsg::GameText { id: id.into(), lines: prepend("rating update:".to_string(), &msg[1..]) }
        }
        (k, _, _) if k.starts_with('.') => match kibitz(&tok[2..], &msg[1..]) {
            Some(line) => OsMsg::Kibitz { id: k.into(), line },
            None => OsMsg::GameText { id: k.into(), lines: prepend(after_spaces(&head, 2).to_string(), &msg[1..]) },
        },
        ("who", Some(n), _) if n.parse::<u32>().is_ok() => {
            OsMsg::Who { users: msg[1..].iter().filter_map(|l| parse_user(l)).collect(), lines: msg }
        }
        ("watch", Some("+"), Some("ERR")) => {
            match os_body(&head).and_then(|b| b.strip_prefix(" watch + ERR not found:")) {
                Some(ids) => OsMsg::WatchMissing { ids: ids.split_whitespace().map(str::to_string).collect() },
                None => OsMsg::Unknown(msg),
            }
        }
        ("finger", Some(t), _)
            if t != "ERR" || os_body(&head).is_some_and(|b| b.starts_with(" finger ERR not found:")) =>
        {
            parse_finger(&msg)
        }
        ("top", _, _) if tok.last().is_some_and(|n| n.parse::<u32>().is_ok()) => {
            OsMsg::Top { ranks: msg[1..].iter().filter_map(|l| Rank::parse(l)).collect(), lines: msg }
        }
        ("look", None, _) => match parse_look(&msg[1..]) {
            Ok(records) => OsMsg::Look(records),
            Err(e) => bad(&e),
        },
        // EXE_Service.C `look` answers a missing key with message _032.
        ("ERR", Some("no"), Some("archived")) => match head.split('\'').nth(1) {
            Some(id) => OsMsg::LookMissing { id: id.into() },
            None => OsMsg::Unknown(msg),
        },
        ("history" | "stored", Some(n), Some(login)) if n.parse::<usize>().is_ok() => {
            let row = if kw == "history" { Archived::history } else { Archived::stored };
            let games = msg[1..].iter().filter_map(|l| row(l)).collect();
            let login = login.to_string();
            if kw == "history" { OsMsg::History { login, games } } else { OsMsg::Stored { login, games } }
        }
        ("look" | "who" | "top", _, _) => bad("invalid reply header"),
        ("update" | "join", _, _) => bad("missing id or move"),
        _ => OsMsg::Unknown(msg),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::othello::{Color, Move};

    fn msg(lines: &[&str]) -> Vec<String> {
        lines.iter().map(|s| s.to_string()).collect()
    }

    fn os(lines: &[&str]) -> OsMsg {
        match parse(msg(lines)) {
            ServerMsg::Os(m) => m,
            other => panic!("not an /os message: {other:?}"),
        }
    }

    #[test]
    fn synchro_result_is_the_mean_of_both_boards_from_the_first_boards_player1() {
        let r = |p1: &str, p2: &str, score| MatchResult { player1: p1.into(), player2: p2.into(), score };
        // Archive .135: `nra -4 edax` and `edax -2 nra`, which `- match` listed as `nra ... -1`.
        assert_eq!(MatchResult::synchro(&r("nra", "edax", -4.), &r("edax", "nra", -2.)), r("nra", "edax", -1.));
        // fixtures/live-synchro.txt: both `end` lines list pbot first and `- match` says -64.00.
        assert_eq!(MatchResult::synchro(&r("pbot", "edax", -64.), &r("pbot", "edax", -64.)), r("pbot", "edax", -64.));
    }

    #[test]
    fn presence_detects_os_arrival_and_departure() {
        assert!(matches!(parse(msg(&[": + /os"])), ServerMsg::Presence { os: Some(true) }));
        assert!(matches!(parse(msg(&[": + alice", ": - /os x"])), ServerMsg::Presence { os: Some(false) }));
        assert!(matches!(parse(msg(&[": + alice"])), ServerMsg::Presence { os: None }));
        assert!(matches!(parse(msg(&[": + /os a b c"])), ServerMsg::Presence { os: None }), ">4 tokens skipped");
        assert!(matches!(parse(msg(&[": hello"])), ServerMsg::Other(_)));
    }

    /// fixtures/live-chat.txt: `help`, `help news` and `tell /os help`.
    #[test]
    fn help_replies_drop_acks_and_continuation_prefixes() {
        let fixture = include_str!("../fixtures/live-chat.txt");
        let mut a = crate::net::Assembler::default();
        let helps: Vec<HelpText> = fixture
            .lines()
            .filter_map(|l| a.feed(l.to_string()).unwrap())
            .filter_map(|m| match parse(m) {
                ServerMsg::Help(h) => Some(h),
                _ => None,
            })
            .collect();
        let titles: Vec<&str> = helps.iter().map(|h| h.title.as_str()).collect();
        assert_eq!(
            titles,
            ["help faq", "help help", "help news", "help help", "help news", "/os: help help"],
            "the first three come unasked at login"
        );
        assert_eq!(helps[4].lines.first().map(String::as_str), Some("GGS News ( also \"help old.news\" )"));
        assert!(helps.iter().all(|h| h.lines.iter().all(|l| !l.contains("+ack"))), "acks dropped");
        assert_eq!(helps[5].lines[0], "Othello Service Server help files", "the | continuation prefix is gone");
    }

    #[test]
    fn hello_skips_open() {
        let h = os_hello();
        assert_eq!(h.last().unwrap(), "tell /os match");
        assert!(h.contains(&"tell /os watch +".to_string()));
        assert!(!h.iter().any(|c| c.contains("open")));
    }

    #[test]
    fn match_lists_skip_blank_rows_and_keep_unparsed_rows_raw() {
        match os(&["/os: match", " .5 1720 alice 1688 bob 8b U ", "", ".13 x"]) {
            OsMsg::MatchList(l) => {
                let MatchEntry::Summary(s) = &l[0] else { panic!("{l:?}") };
                assert_eq!(
                    (s.id.as_str(), s.rating1.as_str(), s.player1.as_str(), s.rating2.as_str(), s.player2.as_str()),
                    (".5", "1720", "alice", "1688", "bob")
                );
                assert_eq!((s.game_type.as_str(), s.mode.as_str()), ("8b", "U"));
                assert_eq!(l[1], MatchEntry::Raw(".13 x".into()));
                assert_eq!(l.len(), 2, "blank line skipped");
            }
            m => panic!("{m:?}"),
        }
        assert!(matches!(MatchEntry::parse("12 1 a 2 b 8 U"), MatchEntry::Raw(_)), "id must start with a dot");
    }

    #[test]
    fn match_add_and_remove_notices_keep_the_id_of_an_unparsed_row() {
        match os(&["/os: + match .14 1600 carol 1500 dave"]) {
            OsMsg::MatchAdd(e) => {
                assert_eq!(e, MatchEntry::Raw(".14 1600 carol 1500 dave".into()), "six fields fall back to raw");
                assert_eq!(e.id(), ".14");
            }
            m => panic!("{m:?}"),
        }
        assert!(
            matches!(os(&["/os: - match .14 carol dave 40-24"]), OsMsg::MatchRemove { id, finished: None } if id == ".14")
        );
        assert_eq!(MatchEntry::Raw(".14 carol".into()).id(), ".14");
    }

    /// Lines copied verbatim from a session with a live GGS (fixtures/live-*.txt).
    #[test]
    fn verbatim_live_os_lines_parse_into_typed_messages() {
        match os(&["/os: match 1/1", "  .4 1720 pbot     1720 edax            8  U 0"]) {
            OsMsg::MatchList(l) => match &l[..] {
                [MatchEntry::Summary(s)] => assert_eq!(
                    (s.id.as_str(), s.player1.as_str(), s.player2.as_str(), s.game_type.as_str(), s.mode.as_str()),
                    (".4", "pbot", "edax", "8", "U")
                ),
                l => panic!("{l:?}"),
            },
            m => panic!("{m:?}"),
        }
        match os(&["/os: match 1/1", "  .5 1720 pbot     1720 edax        s8r10  U 1"]) {
            OsMsg::MatchList(l) => {
                assert!(matches!(&l[..], [MatchEntry::Summary(s)] if s.id == ".5" && s.game_type == "s8r10"))
            }
            m => panic!("{m:?}"),
        }
        assert!(matches!(os(&["/os: match 0/0"]), OsMsg::MatchList(l) if l.is_empty()));
        match os(&["/os: + match .6 1720 pbot 1720 edax 8 U"]) {
            OsMsg::MatchAdd(MatchEntry::Summary(s)) => {
                assert_eq!((s.id.as_str(), s.mode.as_str()), (".6", "U"), "no count column in + match")
            }
            m => panic!("{m:?}"),
        }
        match os(&["/os: - match .4 1720 pbot 1720 edax 8 U -64.00"]) {
            OsMsg::MatchRemove { id, finished: Some(f) } => {
                assert_eq!(id, ".4");
                assert_eq!(f.outcome.describe(), ["pbot -64 edax", "edax wins by 64"]);
                assert_eq!(
                    (f.summary.player1.as_str(), f.summary.game_type.as_str(), f.archive),
                    ("pbot", "8", None),
                    "unrated: not archived"
                );
            }
            m => panic!("{m:?}"),
        }
        let r = MatchResult { player1: "a".into(), player2: "b".into(), score: 2.5 };
        assert_eq!(r.describe(), ["a +2.5 b", "a wins by 2.5"]);
        assert_eq!(MatchResult { score: 0., ..r }.describe()[1], "draw");
        match os(&["/os: end .5.0 ( pbot vs. edax ) -64.00"]) {
            OsMsg::End { id, outcome } => {
                assert_eq!((id.as_str(), outcome.describe()), (".5.0", msg(&["pbot -64 edax", "edax wins by 64"])))
            }
            m => panic!("{m:?}"),
        }
        for (tok, mv, eval, time) in [
            ("d3//6.00", "d3", Some(0.), 6.),
            ("f3/11.00/0.10", "f3", Some(11.), 0.1),
            ("b5/17.00", "b5", Some(17.), 0.),
            ("pa//1.15", "PA", None, 1.15),
        ] {
            match os(&[&format!("/os: update .4 {tok}")]) {
                OsMsg::Update { token, .. } => {
                    assert_eq!((token.mv.to_string().as_str(), token.eval, token.time), (mv, eval, time), "{tok}")
                }
                m => panic!("{m:?}"),
            }
        }
        let who = [
            "/os: who     4      change:    win    draw    loss         match(es)",
            "/os      - 1720.0@350.0 ->  +324.4    +0.0  -324.4 @290.2 ",
            "edax     - 1720.0@350.0 ->  +324.4    +0.0  -324.4 @290.2 .4",
            "pwatch   - 1720.0@350.0",
        ];
        match os(&who) {
            OsMsg::Who { users: u, .. } => {
                let got: Vec<(&str, Option<&str>)> =
                    u.iter().map(|u| (u.name.as_str(), u.playing.as_deref())).collect();
                assert_eq!(got, [("edax", Some(".4")), ("pwatch", None)]);
            }
            m => panic!("{m:?}"),
        }
        match os(&[
            "/os: finger pbot",
            "login  : pbot",
            "dblen  :   0.0 = 0 / 0",
            "rated  : -",
            "request(-) :  : ",
            "stored (-) : 0",
        ]) {
            OsMsg::Finger { settings, missing } => {
                assert_eq!(settings[0].login, "pbot");
                assert_eq!(settings[0].rated, Some(false));
                assert!(missing.is_empty());
            }
            m => panic!("{m:?}"),
        }
        assert!(matches!(parse(msg(&[": + /os 1", ": + edax 1", ": + me 0"])), ServerMsg::Presence { os: Some(true) }));
        assert!(
            matches!(parse(msg(&[": +ack:  tell /os match => tell /os match"])), ServerMsg::Other(_)),
            "acks are not presence"
        );
    }

    #[test]
    fn server_finger_gives_profiles() {
        // From a local server: `finger edax`, then a login that is off with a two-line info, then one it does not know.
        let edax = [
            ": finger",
            "login  : edax",
            "dblen  : 100.0 = 614 / 614",
            "name   : EdaxEngine",
            "info   : ",
            "since  : Thu 01 Oct 2026 13:51:44 UTC",
            "idle   : 00:00:22, on line : 07:50:10",
            "bell   : -t -tc -tg -n -nc -ng -ni -nn",
            "verbose: -news -ack -help -faq",
            "vt100  : -",
            "hear   : +",
            "groups(-) : _client",
            "channs(-) : .chat .edax",
            "notify(-) : /os",
            "ignore(-) : ",
            ": +ack:  finger edax => finger edax",
        ];
        let ServerMsg::Finger { profiles, missing, lines } = parse(msg(&edax)) else { panic!() };
        assert_eq!(lines.len(), edax.len(), "the whole block goes to the log");
        assert!(missing.is_empty());
        assert_eq!(
            profiles,
            [Profile {
                login: "edax".into(),
                name: "EdaxEngine".into(),
                info: vec![],
                seen: Seen::On {
                    since: "Thu 01 Oct 2026 13:51:44 UTC".into(),
                    idle: Some("00:00:22, on line : 07:50:10".into()),
                },
                channels: ".chat .edax".into(),
            }]
        );
        let off = [
            ": finger",
            "login  : ann",
            "name   : ",
            "info   : plays on weekends",
            "info   : 8x8 only",
            "last   : Wed 30 Sep 2026 10:00:00 UTC",
            "after  : 01:00:00",
            ": +ack:  finger ann => finger ann",
        ];
        let ServerMsg::Finger { profiles, .. } = parse(msg(&off)) else { panic!() };
        assert_eq!(
            (profiles[0].info.clone(), &profiles[0].seen),
            (
                vec!["plays on weekends".to_string(), "8x8 only".to_string()],
                &Seen::Off { last: "Wed 30 Sep 2026 10:00:00 UTC".into() }
            )
        );
        let ServerMsg::Finger { profiles, missing, .. } =
            parse(msg(&[": finger ERR not found: nobodyxx", ": +ack:  finger nobodyxx => finger nobodyxx"]))
        else {
            panic!()
        };
        assert_eq!((profiles.len(), missing), (0, vec!["nobodyxx".to_string()]));
        assert!(
            matches!(parse(msg(&["/os: finger pbot", "login  : pbot", "rated  : -"])), ServerMsg::Os(_)),
            "the /os finger stays with /os"
        );
    }

    /// Match.C `~Match`: `%+.2f` after the mode, then the archive counter for a rated match
    /// (fixtures/live-archive.txt).
    #[test]
    fn rated_match_end_has_the_archive_counter_after_the_score() {
        match os(&["/os: - match .23 1402 prated 2038 edax 8 R -64.00  .73"]) {
            OsMsg::MatchRemove { id, finished: Some(f) } => {
                assert_eq!(f.outcome.describe(), ["prated -64 edax", "edax wins by 64"]);
                assert_eq!(
                    (id.as_str(), f.archive.as_deref()),
                    (".23", Some(".73")),
                    "archive key is not the match id"
                );
            }
            m => panic!("{m:?}"),
        }
    }

    /// Unfinished endings from Match.C `~Match`, `adjourn` and `abort`.
    #[test]
    fn aborted_and_adjourned_endings() {
        let removed = |line: &str| match os(&[line]) {
            OsMsg::MatchRemove { finished, .. } => finished.map(|f| (f.outcome.describe(), f.archive)),
            m => panic!("{m:?}"),
        };
        assert_eq!(removed("/os: - match .9 2544 lynx 2505 kitty 8 U aborted"), Some((msg(&["aborted"]), None)));
        assert_eq!(
            removed("/os: - match .9 2544 lynx 2505 kitty 8 R kitty left .18"),
            Some((msg(&["kitty left"]), Some(".18".into())))
        );
        assert_eq!(
            removed("/os: - match .9 2544 lynx 2505 kitty 8 U"),
            Some((msg(&["match finished"]), None)),
            "no tail"
        );
        let ended = |line: &str| match os(&[line]) {
            OsMsg::End { outcome, .. } => outcome.describe(),
            m => panic!("{m:?}"),
        };
        assert_eq!(ended("/os: end .9.1 ( lynx vs. kitty ) player lynx left"), msg(&["lynx left"]));
        assert_eq!(ended("/os: end .9.0 ( lynx vs. kitty ) aborted"), msg(&["aborted"]));
        assert_eq!(ended("/os: end .9.0 ( lynx vs. kitty ) +2.00 "), msg(&["lynx +2 kitty", "lynx wins by 2"]));
    }

    /// Game.C `undo`, `score`, `play`, `handle_timeouts` and Match.C `abort`.
    #[test]
    fn game_notices_read_as_sentences() {
        let text = |line: &str| match os(&[line]) {
            OsMsg::GameText { id, lines } => (id, lines),
            m => panic!("{m:?}"),
        };
        assert_eq!(text("/os: undo .4 pbot is asking"), (".4".into(), msg(&["pbot asks to undo the last move"])));
        assert_eq!(text("/os: abort .5.1 edax is asking").1, msg(&["edax asks to abort the game"]));
        assert_eq!(text("/os: score .4 pbot is offering").1, msg(&["pbot offers to end the game on a score"]));
        assert_eq!(text("/os: timeout .4 pbot (*)").1, msg(&["pbot (black) ran out of time"]));
        assert_eq!(text("/os: fatal-timeout .4 edax (O)").1, msg(&["edax (white) lost on time"]));
        assert_eq!(text("/os: timeout .4 pbot (?)").1, msg(&["pbot ran out of time"]), "komi not yet decided");
        assert_eq!(
            text("/os: trust-violation .4 pbot (*) delta= 10 + 3.2 secs").1,
            msg(&["pbot (black) reported less time than the server measured: delta= 10 + 3.2 secs"])
        );
        assert_eq!(text("/os: comment .4 corner taken").1, msg(&["corner taken"]));
    }

    /// fixtures/live-archive.txt: a single game, a synchro pair and an adjourned game.
    #[test]
    fn look_history_and_stored_replies_parse_from_the_live_fixture() {
        let fixture = include_str!("../fixtures/live-archive.txt");
        let block = |start: &str| -> Vec<String> {
            let i = fixture.lines().position(|l| l.starts_with(start)).unwrap_or_else(|| panic!("{start}"));
            fixture
                .lines()
                .skip(i)
                .take_while(|l| *l != "READY")
                .map(|l| l.strip_prefix('|').unwrap_or(l).to_string())
                .collect()
        };
        let looks: Vec<Vec<GameRecord>> = fixture
            .lines()
            .enumerate()
            .filter(|(_, l)| *l == "/os: look")
            .map(|(i, _)| match parse(vec!["/os: look".into(), fixture.lines().nth(i + 1).unwrap()[1..].to_string()]) {
                ServerMsg::Os(OsMsg::Look(r)) => r,
                m => panic!("{m:?}"),
            })
            .collect();
        let summary: Vec<Vec<_>> = looks
            .iter()
            .map(|rs| {
                rs.iter().map(|r| (r.black.name.as_str(), r.game_type.as_str(), r.result, r.moves.len())).collect()
            })
            .collect();
        assert_eq!(summary[0], [("prated", "8", Some(-64.), 64)]);
        assert_eq!(
            summary[1],
            [("prated", "s8r20", Some(-64.), 52), ("edax", "s8r20", Some(64.), 47)],
            "synchro: two records, colors swapped"
        );
        assert_eq!(summary[2], [("prated", "8", None, 8)], "an adjourned game has RE[?]");
        assert!(matches!(os(&["/os: ERR no archived match '.17' found."]), OsMsg::LookMissing { id } if id == ".17"));
        assert!(matches!(os(&["/os: look", "3 (;BO[4 ---- -O*- -*O- ---- *];)"]), OsMsg::Bad { .. }));
        assert!(
            matches!(os(&["/os: look", "2 (;BO[4 ---- -O*- -*O- ---- *];)"]), OsMsg::Bad { .. }),
            "second record missing"
        );

        let lines = block("/os: history 4 prated");
        let OsMsg::History { login, games } = os(&lines.iter().map(String::as_str).collect::<Vec<_>>()) else {
            panic!()
        };
        assert_eq!((login.as_str(), games.len()), ("prated", 4));
        assert_eq!(
            games[3],
            Archived {
                id: ".73".into(),
                date: "01 Oct 2026 03:42:15".into(),
                player1: "prated".into(),
                player2: "edax".into(),
                score: Some(-64.),
                info: "8".into()
            }
        );
        assert!(matches!(os(&["/os: history 0 parch"]), OsMsg::History { games, .. } if games.is_empty()));
        let lines = block("/os: stored 1 prated");
        match os(&lines.iter().map(String::as_str).collect::<Vec<_>>()) {
            OsMsg::Stored { games, .. } => assert_eq!(
                games.iter().map(|g| (g.id.as_str(), g.player2.as_str(), g.score, g.info.as_str())).collect::<Vec<_>>(),
                [(".69", "edax", None, "8:l")]
            ),
            m => panic!("{m:?}"),
        }
        assert!(matches!(os(&["/os: history ERR not found: nosuchxx"]), OsMsg::Unknown(_)));
    }

    /// `top 8r 24` from fixtures/live-archive.txt.
    #[test]
    fn top_replies_parse_ranks_including_glued_columns() {
        let top = [
            "/os: top       1720.0@352.3      Inactive       AScore    Win   Draw   Loss 2",
            "    1 edax     2072.3@278.0=     00:01:06+@278.0 +61.5      2      0      0",
            "    2 prated   1367.7@278.0=     00:01:06+@278.0 -61.5      0      0      2",
        ];
        match os(&top) {
            OsMsg::Top { ranks: r, .. } => assert_eq!(
                r,
                [
                    Rank {
                        place: 1,
                        name: "edax".into(),
                        rating: 2072.3,
                        stdev: 278.,
                        ascore: 61.5,
                        win: 2,
                        draw: 0,
                        loss: 0
                    },
                    Rank {
                        place: 2,
                        name: "prated".into(),
                        rating: 1367.7,
                        stdev: 278.,
                        ascore: -61.5,
                        win: 0,
                        draw: 0,
                        loss: 2
                    },
                ]
            ),
            m => panic!("{m:?}"),
        }
        assert!(
            matches!(os(&["/os: top          0.0@  0.0      Inactive       AScore    Win   Draw   Loss 0"]), OsMsg::Top { ranks, .. } if ranks.is_empty())
        );
        let glued =
            Rank::parse("   12 kitty    1500.0@ 80.0=  ----------- +@ 80.0-317.8     10      2    123 <=").unwrap();
        assert_eq!((glued.place, glued.stdev, glued.ascore, glued.loss), (12, 80., -317.8, 123), "source-only widths");
    }

    #[test]
    fn archived_margin_is_seen_from_the_given_login() {
        let a = Archived::history(".135 09 Oct 2026 12:05:49 1958 nra 1897 edax -1.0 s8r14").unwrap();
        assert_eq!(a.margin_for("edax"), Some(1.));
        assert_eq!(a.margin_for("nra"), Some(-1.));
        assert_eq!(a.margin_for("bob"), None);
        assert_eq!(Archived::stored(".69 01 Oct 2026 03:35:41 prated edax 8:l").unwrap().margin_for("edax"), None);
    }

    #[test]
    fn non_finite_scores_and_ratings_do_not_enter_typed_messages() {
        let row = "1 edax 2072.3@278.0= 00:01:06+@278.0 +61.5 2 0 0";
        for bad in [
            row.replace("2072.3", "NaN"),
            row.replace("2072.3", "inf"),
            row.replace("278.0=", "-1.0="),
            row.replace("+61.5", "+inf"),
        ] {
            assert!(Rank::parse(&bad).is_none(), "{bad}");
        }
        assert!(Archived::history(".67 01 Oct 2026 03:32:59 1720 prated 1720 edax NaN 8").is_none());
        assert!(matches!(os(&["/os: end .12 (alice vs. bob) NaN"]), OsMsg::End { outcome: Outcome::Other(_), .. }));
        assert!(matches!(os(&["/os: update .12 d3//inf"]), OsMsg::Bad { .. }));
    }

    #[test]
    fn watch_notices_name_the_observer_and_the_game() {
        assert!(matches!(os(&["/os: + eve watch .12"]), OsMsg::WatchAdd { name, id } if name == "eve" && id == ".12"));
        assert!(
            matches!(os(&["/os: - eve watch .12"]), OsMsg::WatchRemove { name, id } if name == "eve" && id == ".12")
        );
    }

    #[test]
    fn join_records_span_lines_and_need_a_board() {
        let m = os(&[
            "/os: join .12",
            "(;GM[Othello]PC[GGS/os]DT[2006.08.17_12:00:00.GMT]PB[alice]PW[bob]RB[1850.00]RW[1720.50]",
            "TI[05:00//02:00]TY[8]RE[?]BO[8 -------- -------- -------- ---O*--- ---*O--- -------- -------- -------- *]",
            "B[d3//3.10]W[c5/-1.23/4.5];)",
        ]);
        match m {
            OsMsg::Join { id, record } => {
                assert_eq!(id, ".12");
                assert_eq!(record.white.name, "bob");
                assert_eq!(record.moves.len(), 2, "lines are joined with separators, not glued");
                assert_eq!(record.start.to_move(), Color::Black);
            }
            m => panic!("{m:?}"),
        }
        assert!(matches!(os(&["/os: join .12 (;PB[a];)"]), OsMsg::Bad { .. }));
        match os(&["/os: join .3", "(;PB[ann", "lee]BO[4 ---- -O*- -*O- ---- *];)"]) {
            OsMsg::Join { record, .. } => {
                assert_eq!(record.black.name, "ann lee", "continuation lines end with a space")
            }
            m => panic!("{m:?}"),
        }
    }

    #[test]
    fn updates_need_a_valid_move_token_and_ignore_extra_fields() {
        match os(&["/os: update .12 f4/0.50/2.73 extra"]) {
            OsMsg::Update { id, token } => {
                assert_eq!(id, ".12");
                assert_eq!((token.mv, token.eval, token.time), (Move::parse("f4").unwrap(), Some(0.5), 2.73));
            }
            m => panic!("{m:?}"),
        }
        assert!(matches!(os(&["/os: update .12"]), OsMsg::Bad { .. }));
        assert!(matches!(os(&["/os: update .12 zz9/x"]), OsMsg::Bad { .. }));
    }

    #[test]
    fn an_end_notice_with_an_unknown_result_keeps_it_as_text() {
        match os(&["/os: end .12 alice-bob", "alice wins 40-24"]) {
            OsMsg::End { id, outcome } => assert_eq!(
                (id.as_str(), outcome.describe()),
                (".12", msg(&["alice-bob", "alice wins 40-24"])),
                "unknown tail kept as text"
            ),
            m => panic!("{m:?}"),
        }
    }

    #[test]
    fn who_lists_users_without_the_service() {
        let m = os(&[
            "/os: who     4      change:    win    draw    loss         match(es)",
            "/os      - 1720.0@350.0 -> +324.4 +0.0 -324.4 @290.2",
            "viewer   - 1720.0@350.0",
            "edax     + 1842.5@120.0 -> +324.4 +0.0 -324.4 @290.2",
            "bot      x 1900.0@ 80.0",
            "nra      + 2016.9@ 85.2 ->  +755.7  +305.3  -145.1 @284.7 .47",
            "odd      ? 1500",
            "short    +",
        ]);
        let OsMsg::Who { users, .. } = m else { panic!("{m:?}") };
        let got: Vec<(&str, UserStatus, &str)> =
            users.iter().map(|u| (u.name.as_str(), u.status, u.rating.as_str())).collect();
        assert_eq!(
            got,
            [
                ("viewer", UserStatus::Busy, "1720.0@350.0"),
                ("edax", UserStatus::Open, "1842.5@120.0"),
                ("bot", UserStatus::Ghost, "1900.0@80.0"),
                ("nra", UserStatus::Open, "2016.9@85.2")
            ]
        );
        assert!(
            matches!(os(&["/os: who is here"]), OsMsg::Bad { reply: Some(ReplyKind::Who), .. }),
            "a malformed who count still identifies the reply"
        );
    }

    #[test]
    fn finger_reads_the_rated_flag_of_each_user_in_a_reply() {
        match os(&["/os: finger edax", "login  : edax", "open   : 1", "rated  : +"]) {
            OsMsg::Finger { settings, .. } => assert_eq!(
                settings,
                [MatchSettings { login: "edax".into(), open: Some(1), rated: Some(true), ..Default::default() }]
            ),
            m => panic!("{m:?}"),
        }
        match os(&[
            "/os: finger a b",
            "login : a",
            "rated : -",
            "login: b",
            "info : x: y",
            "rated: + (since 2020)",
            "rated : +",
        ]) {
            OsMsg::Finger { settings, .. } => assert_eq!(
                settings.iter().map(|s| (s.login.as_str(), s.rated)).collect::<Vec<_>>(),
                [("a", Some(false)), ("b", Some(true))],
                "several users in one reply"
            ),
            m => panic!("{m:?}"),
        }
        assert!(matches!(os(&["/os: finger ERR no such user"]), OsMsg::Unknown(_)));
    }

    #[test]
    fn finger_match_settings_keep_rules_and_distinguish_missing_fields() {
        let m = os(&[
            "/os: finger a",
            "login : a",
            "open : 2",
            "play : .1 .2",
            "rated : +",
            "accept : size == 8 && rated",
            "decline: rand ? mc > 0 : false",
            "ignore (-) : bob eve",
            "/os: finger b",
            "login : b",
            "open : invalid",
            "play :",
            "rated : ?",
            "decline:",
            "/os: finger ERR not found: ghost absent",
        ]);
        let OsMsg::Finger { settings, missing } = m else { panic!("{m:?}") };
        assert_eq!(
            settings,
            [
                MatchSettings {
                    login: "a".into(),
                    open: Some(2),
                    playing: Some(2),
                    rated: Some(true),
                    trust: None,
                    accept: Some("size == 8 && rated".into()),
                    decline: Some("rand ? mc > 0 : false".into()),
                },
                MatchSettings {
                    login: "b".into(),
                    playing: Some(0),
                    decline: Some(String::new()),
                    ..Default::default()
                },
            ]
        );
        assert_eq!(missing, ["ghost", "absent"]);
        let OsMsg::Finger { settings, missing } = os(&["/os: finger ERR not found: ghost"]) else {
            panic!("missing response was not parsed")
        };
        assert!(settings.is_empty());
        assert_eq!(missing, ["ghost"]);
    }

    #[test]
    fn finger_trust_distinguishes_enabled_disabled_and_unknown() {
        let OsMsg::Finger { settings, .. } = os(&[
            "/os: finger a b c d",
            "login : a",
            "trust : +",
            "login : b",
            "trust : -",
            "login : c",
            "trust : ?",
            "login : d",
        ]) else {
            panic!("finger reply was not parsed")
        };
        assert_eq!(settings.iter().map(|s| s.trust).collect::<Vec<_>>(), [Some(true), Some(false), None, None]);
    }

    #[test]
    fn kibitz_needs_a_named_speaker_and_other_game_text_stays_verbatim() {
        let kib = |lines: &[&str]| match os(lines) {
            OsMsg::Kibitz { id, line } => (id, line.from, line.lines),
            m => panic!("{m:?}"),
        };
        assert_eq!(kib(&["/os: .5 A 1720 eve: nice  tiger"]), (".5".into(), "eve".into(), msg(&["nice tiger"])));
        assert_eq!(
            kib(&["/os: .5.1 O 1720 fay: hi", "second line"]),
            (".5.1".into(), "fay".into(), msg(&["hi", "second line"]))
        );
        let text = |lines: &[&str]| match os(lines) {
            OsMsg::GameText { lines, .. } => lines,
            m => panic!("{m:?}"),
        };
        assert_eq!(text(&["/os: .5 Q x eve: hi"]), msg(&["Q x eve: hi"]), "other channels stay verbatim");
        assert_eq!(text(&["/os: .5 A x eve hi"]), msg(&["A x eve hi"]), "no name colon, verbatim");
    }

    /// fixtures/live-chat.txt, seen by pchat1 while pchat2 talks to it.
    #[test]
    fn channel_lines_and_tells_become_chat_and_other_lines_do_not() {
        let chat = |lines: &[&str]| match parse(msg(lines)) {
            ServerMsg::Chat { key, line } => (key, line.from, line.lines),
            m => panic!("{m:?}"),
        };
        let user = |u: &str| ChatKey::User(u.into());
        assert_eq!(
            chat(&[".chat pchat2: reply from b"]),
            (ChatKey::Channel(".chat".into()), "pchat2".into(), msg(&["reply from b"]))
        );
        assert_eq!(chat(&["pchat2: hi a"]), (user("pchat2"), "pchat2".into(), msg(&["hi a"])));
        assert_eq!(chat(&["pchat1: note to self"]).0, user("pchat1"), "a tell to oneself comes back");
        assert_eq!(chat(&["pchat2: two", "lines"]).2, msg(&["two", "lines"]), "a multi-line tell keeps its lines");
        for other in [
            ": ERR command 'frobnicate' not recognized.",
            ": who    5        idle      online",
            "/ams: hi",
            "Multiplexors on:",
            "a b: c",
        ] {
            assert!(matches!(parse(msg(&[other])), ServerMsg::Other(_)), "{other}");
        }
    }

    #[test]
    fn game_text_and_rating_updates_are_kept_per_game() {
        match os(&["/os: .12 alice: good luck", "more"]) {
            OsMsg::GameText { id, lines } => {
                assert_eq!((id.as_str(), lines), (".12", msg(&["alice: good luck", "more"])))
            }
            m => panic!("{m:?}"),
        }
        match os(&["/os: rating_update .12", "alice 1855"]) {
            OsMsg::GameText { lines, .. } => assert_eq!(lines, msg(&["rating update:", "alice 1855"])),
            m => panic!("{m:?}"),
        }
        assert!(matches!(os(&["/os: frobnicate 1"]), OsMsg::Unknown(_)));
    }

    #[test]
    fn close_deviation_drops_every_space_after_an_at() {
        assert_eq!(close_deviation("a 2016.9@  85.2 +@ @ 1"), "a 2016.9@85.2 +@@1");
        assert_eq!(close_deviation(&format!("x@{}y", " ".repeat(60_000))), "x@y");
    }
}
