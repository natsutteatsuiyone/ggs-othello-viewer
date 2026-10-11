use std::collections::{BTreeMap, BTreeSet};

use crate::{
    game::{Game, GameKey},
    lobby::{Lobby, ViewMode, match_of},
    proto::MatchEntry,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum MatchContent {
    Boards,
    ArchiveLoading,
    ArchiveUnavailable,
    Stopped,
    Replaced,
}

/// A child window's accepted match generation. Changes are reconciled when the
/// window opens or the lobby changes, independently of whether a frame is drawn.
pub(super) struct MatchState {
    pub(super) id: String,
    pub(super) synchro: bool,
    pub(super) historical: bool,
    pub(super) mode: ViewMode,
    pub(super) selected: usize,
    pub(super) content: MatchContent,
    anchors: BTreeMap<String, GameKey>,
    /// Finished predecessors of an explicitly opened live match number.
    waiting_for: BTreeSet<GameKey>,
}

impl MatchState {
    pub(super) fn open(requested: &str, live: bool, lobby: &Lobby) -> Self {
        let id = match_of(requested).to_string();
        let held = lobby.games.values().filter(|game| match_of(&game.id) == id);
        let historical = !live && held.clone().any(|game| game.match_ended);
        let waiting_for =
            if live { held.filter(|game| game.match_ended).map(|game| game.key).collect() } else { BTreeSet::new() };
        let kind = if historical { held_match_is_synchro(lobby, &id) } else { match_is_synchro(lobby, &id) };
        let synchro = requested != id || kind.unwrap_or(false);
        let mut state = Self {
            id,
            synchro,
            historical,
            waiting_for,
            mode: if synchro { ViewMode::Synchro } else { ViewMode::Focus },
            selected: 0,
            anchors: BTreeMap::new(),
            content: MatchContent::Boards,
        };
        state.reconcile(lobby);
        state
    }

    /// An explicit reopen may accept a replacement. Returns whether the caller
    /// must discard board browsing state; same-generation reopens preserve it.
    pub(super) fn reopen(&mut self, requested: Self, lobby: &Lobby) -> bool {
        debug_assert_eq!(self.id, requested.id);
        let replaced =
            self.anchors.iter().any(|(id, anchor)| lobby.games.get(id).is_some_and(|game| *anchor != game.key));
        let predecessor = self.anchors.values().any(|key| requested.waiting_for.contains(key));
        let reset = replaced || predecessor || self.synchro != requested.synchro;
        if reset {
            self.anchors = requested.anchors;
            self.selected = 0;
            self.mode = requested.mode;
        } else {
            // A pending window also accepts the generation observed by the open
            // action, rather than a later join received before the deferred UI work.
            for (id, key) in requested.anchors {
                self.anchors.entry(id).or_insert(key);
            }
        }
        self.synchro = requested.synchro;
        self.waiting_for = requested.waiting_for;
        self.historical = requested.historical;
        self.reconcile(lobby);
        reset
    }

    pub(super) fn reconcile(&mut self, lobby: &Lobby) {
        let kind =
            if self.historical { held_match_is_synchro(lobby, &self.id) } else { match_is_synchro(lobby, &self.id) };
        let changed_kind = !self.anchors.is_empty() && kind.is_some_and(|kind| kind != self.synchro);
        if self.anchors.is_empty()
            && let Some(kind) = kind
        {
            if kind != self.synchro {
                self.mode = if kind { ViewMode::Synchro } else { ViewMode::Focus };
                self.selected = 0;
            }
            self.synchro = kind;
        }
        let ids = self.board_ids();
        let games: Vec<_> =
            ids.iter().map(|id| lobby.games.get(id).filter(|game| !self.waiting_for.contains(&game.key))).collect();
        let missing = games.iter().all(Option::is_none);
        let replaced =
            games.iter().flatten().any(|game| self.anchors.get(&game.id).is_some_and(|anchor| *anchor != game.key));
        self.content = if missing && self.id.starts_with("archive ") {
            if lobby.archive_pending(&self.id) {
                MatchContent::ArchiveLoading
            } else {
                MatchContent::ArchiveUnavailable
            }
        } else if missing && !lobby.is_watched(&self.id) {
            MatchContent::Stopped
        } else if replaced || changed_kind {
            MatchContent::Replaced
        } else {
            for game in games.into_iter().flatten() {
                self.anchors.entry(game.id.clone()).or_insert(game.key);
            }
            MatchContent::Boards
        };
    }

    pub(super) fn selected_id(&self) -> String {
        if self.synchro { format!("{}.{}", self.id, self.selected) } else { self.id.clone() }
    }

    fn board_ids(&self) -> Vec<String> {
        if self.synchro { vec![format!("{}.0", self.id), format!("{}.1", self.id)] } else { vec![self.id.clone()] }
    }

    /// Only expose accepted boards, even if the lobby changes before its observer
    /// runs. Reading games never registers a new generation.
    pub(super) fn game<'a>(&self, lobby: &'a Lobby, id: &str) -> Option<&'a Game> {
        lobby
            .games
            .get(id)
            .filter(|game| self.anchors.get(id) == Some(&game.key) && !self.waiting_for.contains(&game.key))
    }

    pub(super) fn games<'a>(&self, lobby: &'a Lobby) -> Vec<Option<&'a Game>> {
        self.board_ids().iter().map(|id| self.game(lobby, id)).collect()
    }
}

fn match_is_synchro(lobby: &Lobby, id: &str) -> Option<bool> {
    lobby
        .matches
        .iter()
        .find_map(|entry| match entry {
            MatchEntry::Summary(m) if match_of(&m.id) == id => Some(m.game_type.to_ascii_lowercase().starts_with('s')),
            _ => None,
        })
        .or_else(|| held_match_is_synchro(lobby, id))
        .or_else(|| lobby.matches.iter().any(|entry| match_of(entry.id()) == id && entry.id() != id).then_some(true))
}

fn held_match_is_synchro(lobby: &Lobby, id: &str) -> Option<bool> {
    let games: Vec<_> =
        lobby.games.values().chain(lobby.restoring_games.values()).filter(|game| match_of(&game.id) == id).collect();
    (!games.is_empty())
        .then(|| games.iter().any(|game| game.id != id || game.game_type.to_ascii_lowercase().starts_with('s')))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ggf::{GameRecord, tests::SPEC_JOIN},
        lobby::{CommandSource, OsState},
        proto::{self, OsMsg, Outcome},
    };
    use std::time::Instant;

    fn join(lobby: &mut Lobby, id: &str, kind: &str, date: Option<&str>) {
        let mut record = GameRecord::parse(SPEC_JOIN).unwrap();
        record.game_type = kind.into();
        if let Some(date) = date {
            record.date = Some(date.into());
        }
        assert!(lobby.apply(OsMsg::Join { id: id.into(), record: Box::new(record) }, Instant::now()).is_empty());
    }

    fn listed(lobby: &mut Lobby, id: &str, kind: &str) {
        lobby.apply(OsMsg::MatchAdd(MatchEntry::parse(&format!("{id} 1800 alice 1900 bob {kind} U"))), Instant::now());
    }

    fn end_match(lobby: &mut Lobby, id: &str) {
        lobby.apply(OsMsg::MatchRemove { id: id.into(), finished: None }, Instant::now());
    }

    fn watch(lobby: &mut Lobby, id: &str) {
        lobby.os = OsState::Present;
        lobby.sent(&proto::tell(&format!("watch + {id}")), CommandSource::App);
    }

    #[test]
    fn opening_binds_the_generation_without_drawing_a_frame() {
        let mut lobby = Lobby::default();
        join(&mut lobby, ".12", "8", None);
        let mut state = MatchState::open(".12", true, &lobby);
        let accepted = state.game(&lobby, ".12").unwrap().key;
        listed(&mut lobby, ".12", "8");
        join(&mut lobby, ".12", "8", Some("next match"));
        assert!(state.game(&lobby, ".12").is_none(), "unreconciled replacements cannot become visible");
        state.reconcile(&lobby);
        assert_eq!(state.content, MatchContent::Replaced);
        assert_eq!(state.anchors[".12"], accepted);
        assert!(state.reopen(MatchState::open(".12", true, &lobby), &lobby));
        assert_eq!(state.content, MatchContent::Boards);
        assert_ne!(state.game(&lobby, ".12").unwrap().key, accepted);
    }

    #[test]
    fn successive_joins_cannot_replace_a_loading_window_before_its_first_frame() {
        let mut lobby = Lobby::default();
        watch(&mut lobby, ".12");
        let mut state = MatchState::open(".12", true, &lobby);
        assert!(state.game(&lobby, ".12").is_none());
        join(&mut lobby, ".12", "8", None);
        state.reconcile(&lobby);
        let accepted = state.game(&lobby, ".12").unwrap().key;
        end_match(&mut lobby, ".12");
        join(&mut lobby, ".12", "8", None);
        state.reconcile(&lobby);
        assert_eq!(state.content, MatchContent::Replaced);
        assert_eq!(state.anchors[".12"], accepted);
    }

    #[test]
    fn a_live_open_waits_for_a_recycled_number_in_new_and_existing_windows() {
        for existing in [false, true] {
            let mut lobby = Lobby::default();
            join(&mut lobby, ".12", "8", None);
            let mut state = MatchState::open(".12", false, &lobby);
            end_match(&mut lobby, ".12");
            listed(&mut lobby, ".12", "8");
            watch(&mut lobby, ".12");
            let request = MatchState::open(".12", true, &lobby);
            if existing {
                assert!(state.reopen(request, &lobby));
            } else {
                state = request;
            }
            assert_eq!(state.content, MatchContent::Boards);
            assert!(state.games(&lobby).iter().all(Option::is_none));
            join(&mut lobby, ".12", "8", None);
            state.reconcile(&lobby);
            assert_eq!(state.content, MatchContent::Boards);
            assert!(state.game(&lobby, ".12").is_some());
        }
    }

    #[test]
    fn each_synchro_half_waits_for_its_own_fresh_generation() {
        for first_join_before_open in [false, true] {
            let mut lobby = Lobby::default();
            for id in [".20.0", ".20.1"] {
                join(&mut lobby, id, "s8", None);
            }
            end_match(&mut lobby, ".20");
            listed(&mut lobby, ".20", "s8");
            watch(&mut lobby, ".20");
            if first_join_before_open {
                join(&mut lobby, ".20.0", "s8", Some("next match"));
            }
            let mut state = MatchState::open(".20", true, &lobby);
            assert_eq!(state.games(&lobby)[0].is_some(), first_join_before_open);
            assert!(state.games(&lobby)[1].is_none());
            for id in [".20.0", ".20.1"] {
                join(&mut lobby, id, "s8", Some("next match"));
            }
            state.reconcile(&lobby);
            assert_eq!(state.content, MatchContent::Boards);
            assert!(state.games(&lobby).iter().all(Option::is_some));
        }
    }

    #[test]
    fn a_bound_shape_changes_only_on_explicit_reopen() {
        let mut lobby = Lobby::default();
        join(&mut lobby, ".12", "8", None);
        let mut state = MatchState::open(".12", true, &lobby);
        listed(&mut lobby, ".12", "s8");
        for id in [".12.0", ".12.1"] {
            join(&mut lobby, id, "s8", Some("synchro"));
        }
        state.reconcile(&lobby);
        assert_eq!(state.content, MatchContent::Replaced);
        assert!(!state.synchro);
        assert!(state.reopen(MatchState::open(".12", true, &lobby), &lobby));
        assert!(state.synchro);
        assert_eq!(state.mode, ViewMode::Synchro);
        state.mode = ViewMode::Focus;
        state.selected = 1;
        listed(&mut lobby, ".12", "8");
        state.reconcile(&lobby);
        assert_eq!(state.content, MatchContent::Replaced);
        assert!(state.synchro);
        assert!(state.reopen(MatchState::open(".12", true, &lobby), &lobby));
        join(&mut lobby, ".12", "8", Some("normal"));
        state.reconcile(&lobby);
        assert!(!state.synchro);
        assert_eq!(state.mode, ViewMode::Focus);
        assert_eq!(state.selected, 0);
        assert_eq!(state.content, MatchContent::Boards);
    }

    #[test]
    fn an_unbound_shape_can_be_discovered_before_any_board_arrives() {
        let mut lobby = Lobby::default();
        watch(&mut lobby, ".20");
        let mut state = MatchState::open(".20", true, &lobby);
        listed(&mut lobby, ".20", "s8");
        state.reconcile(&lobby);
        assert!(state.synchro);
        assert_eq!(state.mode, ViewMode::Synchro);
        state.mode = ViewMode::Focus;
        state.selected = 1;
        listed(&mut lobby, ".20", "8");
        state.reconcile(&lobby);
        assert!(!state.synchro);
        assert_eq!(state.mode, ViewMode::Focus);
        assert_eq!(state.selected, 0);
    }

    #[test]
    fn one_finished_synchro_half_remains_part_of_the_current_match() {
        let mut lobby = Lobby::default();
        for id in [".20.0", ".20.1"] {
            join(&mut lobby, id, "s8", None);
        }
        lobby.apply(OsMsg::End { id: ".20.0".into(), outcome: Outcome::finished("done".into()) }, Instant::now());
        let mut state = MatchState::open(".20", true, &lobby);
        assert!(state.games(&lobby).iter().all(Option::is_some));
        end_match(&mut lobby, ".20");
        state.reconcile(&lobby);
        assert_eq!(state.content, MatchContent::Boards);
        assert!(state.games(&lobby).iter().all(Option::is_some));
    }

    #[test]
    fn a_historical_open_follows_held_boards_instead_of_a_recycled_listing() {
        let mut lobby = Lobby::default();
        join(&mut lobby, ".12", "8", None);
        end_match(&mut lobby, ".12");
        listed(&mut lobby, ".12", "s8");
        let state = MatchState::open(".12", false, &lobby);
        assert!(state.historical);
        assert!(!state.synchro);
        assert_eq!(state.content, MatchContent::Boards);
        assert!(state.game(&lobby, ".12").is_some());
        assert!(!lobby.is_watched(".12"));
    }

    #[test]
    fn archive_loading_and_failure_do_not_require_a_frame() {
        for synchro in [false, true] {
            let mut lobby = Lobby::default();
            lobby.sent(&proto::tell("look .73"), CommandSource::App);
            let mut state = MatchState::open("archive .73", false, &lobby);
            assert_eq!(state.content, MatchContent::ArchiveLoading);
            let mut record = GameRecord::parse(SPEC_JOIN).unwrap();
            if synchro {
                record.game_type = "s8".into();
            }
            let records = if synchro { vec![record.clone(), record] } else { vec![record] };
            lobby.apply(OsMsg::Look(records), Instant::now());
            state.reconcile(&lobby);
            assert_eq!(state.content, MatchContent::Boards);
            assert_eq!(state.synchro, synchro);
            assert!(state.games(&lobby).iter().all(Option::is_some));
            assert!(lobby.watched.is_empty());
        }
        let mut lobby = Lobby::default();
        lobby.sent(&proto::tell("look .74"), CommandSource::App);
        let mut state = MatchState::open("archive .74", false, &lobby);
        lobby.apply(OsMsg::LookMissing { id: ".74".into() }, Instant::now());
        state.reconcile(&lobby);
        assert_eq!(state.content, MatchContent::ArchiveUnavailable);
    }

    #[test]
    fn same_generation_updates_and_reopens_preserve_the_view_selection() {
        let mut lobby = Lobby::default();
        for id in [".20.0", ".20.1"] {
            join(&mut lobby, id, "s8", None);
        }
        let mut state = MatchState::open(".20", true, &lobby);
        state.mode = ViewMode::Focus;
        state.selected = 1;
        let accepted = state.anchors.clone();
        join(&mut lobby, ".20.1", "s8", Some("later send time"));
        state.reconcile(&lobby);
        assert!(!state.reopen(MatchState::open(".20", true, &lobby), &lobby));
        assert_eq!(state.anchors, accepted);
        assert_eq!(state.mode, ViewMode::Focus);
        assert_eq!(state.selected, 1);
        lobby.suspend();
        state.reconcile(&lobby);
        for id in [".20.0", ".20.1"] {
            join(&mut lobby, id, "s8", Some("reconnect send time"));
        }
        state.reconcile(&lobby);
        assert_eq!(state.content, MatchContent::Boards);
        assert_eq!(state.anchors, accepted);
        assert_eq!(state.mode, ViewMode::Focus);
        assert_eq!(state.selected, 1);
        lobby.os = OsState::Present;
        lobby.sent(&proto::tell("watch - .20"), CommandSource::App);
        state.reconcile(&lobby);
        assert_eq!(state.content, MatchContent::Stopped);
    }

    #[test]
    fn a_deferred_reopen_keeps_the_generation_accepted_by_the_open_action() {
        for previously_bound in [false, true] {
            let mut lobby = Lobby::default();
            watch(&mut lobby, ".12");
            if previously_bound {
                join(&mut lobby, ".12", "8", Some("previous"));
            }
            let mut state = MatchState::open(".12", true, &lobby);
            listed(&mut lobby, ".12", "8");
            join(&mut lobby, ".12", "8", Some("requested"));
            let requested = MatchState::open(".12", true, &lobby);
            let accepted = requested.anchors[".12"];
            // UI window creation/activation is deferred, but game identity is not.
            listed(&mut lobby, ".12", "8");
            join(&mut lobby, ".12", "8", Some("later"));
            state.reopen(requested, &lobby);
            assert_eq!(state.content, MatchContent::Replaced);
            assert_eq!(state.anchors[".12"], accepted);
            assert!(state.game(&lobby, ".12").is_none());
        }
    }
}
