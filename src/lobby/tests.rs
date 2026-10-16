use super::*;
use crate::game::{GameView, GameViews, Nav, tests::tok};
use crate::ggf::tests::SPEC_JOIN;
use crate::othello::Color;
use std::time::Duration;

fn execute(lobby: &mut Lobby, commands: Vec<AppCommand>) -> Vec<String> {
    let mut lines = Vec::new();
    for command in commands {
        lobby.send_app(command, |line| {
            lines.push(line.to_string());
            true
        });
    }
    lines
}

#[test]
fn rejected_commands_keep_existing_watches_and_do_not_reserve_archive_replies() {
    let mut lobby = Lobby::default();
    assert!(lobby.send_app(AppCommand::Watch(".99".into()), |_| true));
    assert!(!lobby.send_app(AppCommand::Unwatch(".99".into()), |_| false));
    assert!(lobby.is_watched(".99"), "a rejected stop must not release the watch");
    assert!(!lobby.send_app(AppCommand::Look(".73".into()), |_| false));
    assert!(lobby.send_app(AppCommand::Look(".74".into()), |_| true));
    lobby.apply(OsMsg::Look(vec![GameRecord::parse(SPEC_JOIN).unwrap()]), Instant::now());
    assert!(lobby.games.contains_key("archive .74"), "the reply belongs to the accepted request");
    assert!(!lobby.games.contains_key("archive .73"));
}

fn lines(commands: Vec<AppCommand>) -> Vec<String> {
    commands.into_iter().map(|command| command.line()).collect()
}

fn look(lobby: &mut Lobby, key: &str) -> Vec<String> {
    let commands = lobby.look(key);
    execute(lobby, commands)
}

fn watch(lobby: &mut Lobby, id: &str) -> Vec<String> {
    let commands = lobby.watch(id);
    execute(lobby, commands)
}

fn unwatch(lobby: &mut Lobby, id: &str) -> Vec<String> {
    let commands = lobby.unwatch(id);
    execute(lobby, commands)
}

fn retain_watches(lobby: &mut Lobby, keep: impl Fn(&str) -> bool) -> Vec<String> {
    let commands = lobby.retain_watches(keep);
    execute(lobby, commands)
}

fn who(lobby: &mut Lobby, rating: RatingType) -> String {
    execute(lobby, vec![AppCommand::Who(rating)]).pop().unwrap()
}

fn top(lobby: &mut Lobby, rating: RatingType) -> String {
    execute(lobby, vec![AppCommand::Top(rating)]).pop().unwrap()
}

fn kibitz(lobby: &mut Lobby, id: &str, me: &str, text: &str, observers_only: bool) -> Vec<String> {
    let command = AppCommand::Kibitz { id: id.into(), me: me.into(), text: text.into(), observers_only };
    execute(lobby, vec![command])
}

#[test]
fn prepared_commands_do_not_create_subscriptions_or_pending_replies() {
    let lobby = Lobby::default();
    drop(lobby.request_watch(".12"));
    drop(lobby.request_look(".73"));
    assert!(lobby.watched.is_empty() && lobby.looks.is_empty());
    assert!(lobby.who_asked.is_empty() && lobby.top_asked.is_empty());
    assert!(lobby.unacked.is_empty());
    assert_eq!(lobby.next_seq, 0);
}

#[test]
fn duplicate_plans_are_registered_and_enqueued_only_once() {
    let mut lobby = Lobby::default();
    let mut commands = lobby.request_watch(".20.0");
    commands.extend(lobby.request_watch(".20.1"));
    commands.extend(lobby.request_look(".73"));
    commands.extend(lobby.request_look(".73"));
    assert_eq!(execute(&mut lobby, commands), ["tell /os watch + .20", "tell /os look .73"]);
    assert_eq!(lobby.watched, [".20"]);
    assert_eq!(lobby.looks, [(1, ".73".into())]);
    assert_eq!(lobby.unacked.len(), 2);
    let mut commands = lobby.unwatch(".20");
    commands.extend(lobby.unwatch(".20"));
    assert!(lobby.is_watched(".20"), "stopping is committed at the same boundary as watching");
    assert_eq!(execute(&mut lobby, commands), ["tell /os watch - .20"]);
    assert!(!lobby.is_watched(".20"));
    assert_eq!(lobby.next_seq, 3);
}

#[test]
fn reconnect_plans_keep_intent_until_their_send_boundary() {
    let mut lobby = Lobby::default();
    watch(&mut lobby, ".12");
    look(&mut lobby, ".73");
    lobby.connection_lost();
    assert!(lobby.looks.is_empty());
    drop(lobby.reconnect_commands());
    assert!(lobby.reopen_archives.contains(".73"), "dropping an unsent restoration plan must not lose the request");
    let commands = lobby.reconnect_commands();
    assert_eq!(execute(&mut lobby, commands), ["tell /os watch + .12", "tell /os look .73"]);
    assert!(!lobby.reopen_archives.contains(".73"));
    assert!(lobby.archive_pending("archive .73"));
    assert_eq!(lobby.watched, [".12"], "restoration resends the intended watch despite ordinary watch deduplication");
}

#[test]
fn explicit_app_watch_intent_survives_os_being_away() {
    let mut lobby = Lobby::default();
    lobby.suspend();
    let commands = lobby.request_watch(".12");
    assert_eq!(execute(&mut lobby, commands), ["tell /os watch + .12"]);
    assert_eq!(lobby.watched, [".12"]);
    assert_eq!(lines(lobby.reconnect_commands()), ["tell /os watch + .12"]);
}

#[test]
fn explicit_app_stop_while_os_is_away_does_not_restore_abandoned_watches() {
    for retain_record in [false, true] {
        let mut lobby = Lobby::default();
        watch(&mut lobby, ".12");
        lobby.suspend();
        let commands = if retain_record { lobby.retain_watches(|_| false) } else { lobby.unwatch(".12") };
        assert!(lobby.is_watched(".12"), "preparing a stop retains intent until committed");
        assert_eq!(execute(&mut lobby, commands), ["tell /os watch - .12"]);
        assert!(lobby.reconnect_commands().is_empty());
    }
}

pub(crate) fn joined(now: Instant) -> Lobby {
    let mut lobby = Lobby::default();
    join(&mut lobby, ".12", now);
    lobby
}

#[test]
fn joined_running_clock_is_marked_until_the_next_move() {
    let now = Instant::now();
    let mut lobby = joined(now);
    let g = &lobby.games[".12"];
    assert_eq!(g.clock_text(Color::Black, None, now), "~+0:04:56", "side to move: time on this move unknown");
    assert_eq!(g.clock_text(Color::White, None, now), "+0:04:55", "waiting side is exact");
    assert_eq!(g.clock_text(Color::Black, Some(1), now), "+0:04:56", "history values are exact");
    lobby.apply(OsMsg::Update { id: ".12".into(), token: tok("e6//7") }, now);
    let g = &lobby.games[".12"];
    assert_eq!(g.clock_text(Color::White, None, now), "+0:04:55", "exact after a live move");
}

#[test]
fn synchro_clock_waits_for_confirmed_move_time() {
    let now = Instant::now();
    let mut record = GameRecord::parse(SPEC_JOIN).unwrap();
    record.game_type = "s8r10".into();
    let mut game = Game::from_record(".20.0".into(), record, now).unwrap();

    let later = now + Duration::from_secs(20);
    assert_eq!(
        game.clock_text(Color::Black, None, later),
        "+0:04:56",
        "a synchro join shows the last confirmed balance without an estimate mark"
    );
    assert_eq!(
        game.move_elapsed_text(Color::Black, None, later),
        None,
        "the current move began before this viewer joined"
    );

    game.play(tok("e6//3"), now).unwrap();

    assert_eq!(game.clock_text(Color::Black, None, later), "+0:04:53", "only the reported three seconds are charged");
    let confirmed = game.clock_text(Color::White, None, now);
    assert_eq!(confirmed, "+0:04:55", "the move update confirms the waiting player's balance");
    assert_eq!(
        game.clock_text(Color::White, None, later),
        confirmed,
        "a synchro clock must not charge the hidden synchronization wait"
    );
    assert_eq!(game.move_elapsed_text(Color::White, None, later).as_deref(), Some("0:20"));
    assert_eq!(game.move_elapsed_text(Color::White, None, now + Duration::from_secs(3661)).as_deref(), Some("1:01:01"));
    assert_eq!(game.move_elapsed_text(Color::Black, None, later), None, "only the active side gets an elapsed time");
    assert_eq!(game.move_elapsed_text(Color::White, Some(3), later), None, "history has no live elapsed time");

    game.play(tok("f3//4"), later).unwrap();
    assert_eq!(
        game.move_elapsed_text(Color::Black, None, later).as_deref(),
        Some("0:00"),
        "a confirmed move resets elapsed time"
    );
    assert_eq!(game.move_elapsed_text(Color::Black, None, later + Duration::from_secs(2)).as_deref(), Some("0:02"));

    game.phase = Phase::Over(Outcome::Aborted);
    assert_eq!(
        game.move_elapsed_text(Color::Black, None, later + Duration::from_secs(2)),
        None,
        "ended games have no elapsed time"
    );

    let mut record = GameRecord::parse(SPEC_JOIN).unwrap();
    record.game_type = "s8r10".into();
    let archived = Game::archived("archive .20.0".into(), record, now).unwrap();
    assert_eq!(archived.move_elapsed_text(Color::Black, None, later), None, "archived games have no elapsed time");
}

#[test]
fn ggf_round_trips_through_the_parser() {
    let now = Instant::now();
    let komi = "(;GM[Othello]PB[ann]PW[ben]RB[1500.5]RW[1499]TB[10:00/5/]TW[5:00//1:00]TY[8k]KB[d3/1/5.5]KW[x/-2/7]KM[-2]\
                    BO[8 -------- -------- -------- ---O*--- ---*O--- -------- -------- -------- O];)";
    let octagon = format!("(;TY[88]BO[88 {}O*{}*O{} *]B[d5];)", "-".repeat(38), "-".repeat(8), "-".repeat(38));
    for text in [SPEC_JOIN, komi, &octagon] {
        let rec = GameRecord::parse(text).unwrap();
        let g = Game::from_record(".1".into(), rec.clone(), now).unwrap();
        assert_eq!(GameRecord::parse(&g.to_ggf()).unwrap(), rec, "{text}");
    }
    for reason in ["r", "s", "t"] {
        let archived = SPEC_JOIN.replace("RE[?]", &format!("RE[-6.000:{reason}]"));
        let rec = GameRecord::parse(&archived).unwrap();
        assert_eq!(rec.date.as_deref(), Some("2006.08.17_12:00:00.GMT"));
        assert_eq!(rec.result_reason.as_deref(), Some(reason));
        let g = Game::archived(archive_id(".9"), rec.clone(), now).unwrap();
        assert_eq!(GameRecord::parse(&g.to_ggf()).unwrap(), rec, "DT and the RE suffix survive export");
    }
    let mut lobby = joined(now);
    lobby.apply(OsMsg::Update { id: ".12".into(), token: tok("e6//7") }, now);
    let score =
        |p1: &str, p2: &str, s| Outcome::Score(proto::MatchResult { player1: p1.into(), player2: p2.into(), score: s });
    lobby.apply(OsMsg::End { id: ".12".into(), outcome: score("bob", "alice", 6.) }, now);
    let ended = GameRecord::parse(&lobby.games[".12"].to_ggf()).unwrap();
    assert_eq!((ended.result, ended.moves.len()), (Some(-6.), 3), "bob is white, so black's margin is -6");
    lobby.apply(OsMsg::End { id: ".12".into(), outcome: score("alice", "bob", 6.) }, now);
    assert_eq!(GameRecord::parse(&lobby.games[".12"].to_ggf()).unwrap().result, Some(6.));
}

/// Checks a record copied out of the app: `GGF_FILE=game.ggf cargo test -- --ignored ggf_file`.

#[test]
fn each_game_keeps_its_view_across_focus_changes_and_updates() {
    let mut views = GameViews::default();
    let now = Instant::now();
    let mut lobby = joined(now);
    views.navigate(lobby.focused().unwrap(), Nav::Prev);
    assert_eq!(lobby.focused().and_then(|g| views.get(g).cursor), Some(1));
    lobby.apply(OsMsg::Update { id: ".12".into(), token: tok("e6//7") }, now);
    assert_eq!(
        lobby.focused().and_then(|g| views.get(g).cursor),
        Some(1),
        "a new move does not move a browsing cursor"
    );
    assert_eq!(lobby.focused().unwrap().board(lobby.focused().and_then(|g| views.get(g).cursor)).count(), (4, 1));
    views.navigate(lobby.focused().unwrap(), Nav::Next);
    views.navigate(lobby.focused().unwrap(), Nav::Next);
    assert_eq!(lobby.focused().and_then(|g| views.get(g).cursor), None);
    views.navigate(lobby.focused().unwrap(), Nav::First);
    lobby.focus = Some(".12".into());
    assert_eq!(lobby.focused().and_then(|g| views.get(g).cursor), Some(0), "refocusing the same game keeps the cursor");
    lobby.focus = Some(".13".into());
    assert_eq!(lobby.focused().and_then(|g| views.get(g).cursor), None);
    lobby.apply(OsMsg::Join { id: ".13".into(), record: Box::new(GameRecord::parse(SPEC_JOIN).unwrap()) }, now);
    views.navigate(lobby.focused().unwrap(), Nav::Prev);
    let other = GameView { cursor: Some(1), flipped: true, hints: false };
    *views.get_mut(lobby.focused().unwrap()) = other;
    lobby.focus = Some(".12".into());
    assert_eq!(views.get(lobby.focused().unwrap()), GameView { cursor: Some(0), ..Default::default() });
    lobby.show_view(ViewMode::Synchro);
    lobby.show(".13");
    assert_eq!(views.get(lobby.focused().unwrap()), other);
    lobby.apply(OsMsg::Join { id: ".13".into(), record: Box::new(GameRecord::parse(SPEC_JOIN).unwrap()) }, now);
    assert_eq!(views.get(lobby.focused().unwrap()), other, "a refreshed record preserves the view");
}

#[test]
fn recycled_match_ids_do_not_keep_the_previous_games_view() {
    let mut views = GameViews::default();
    let now = Instant::now();
    for (reconnect, change) in [(false, 0), (false, 1), (false, 2), (true, 0), (true, 1), (true, 2)] {
        let mut lobby = joined(now);
        views.navigate(lobby.focused().unwrap(), Nav::Prev);
        views.get_mut(lobby.focused().unwrap()).flipped = true;
        lobby.games.get_mut(".12").unwrap().observers.insert("eve".into());
        lobby.games.get_mut(".12").unwrap().text.push("old match".into());
        lobby.focus = Some(".13".into());
        let mut record = GameRecord::parse(SPEC_JOIN).unwrap();
        match change {
            0 => {
                lobby.apply(OsMsg::MatchAdd(MatchEntry::parse(".12 1850 alice 1720 bob 8 U")), now);
            }
            1 => record.black.name = "carol".into(),
            _ => {
                lobby.apply(OsMsg::MatchRemove { id: ".12".into(), finished: None }, now);
                record.date = None;
                lobby.games.get_mut(".12").unwrap().date = None;
            }
        }
        if reconnect {
            let old = lobby.games.remove(".12").unwrap();
            lobby.restoring_games.insert(".12".into(), old);
        }
        lobby.apply(OsMsg::Join { id: ".12".into(), record: Box::new(record) }, now);
        lobby.focus = Some(".12".into());
        let game = lobby.focused().unwrap();
        assert_eq!(views.get(game), GameView::default(), "a different match ({change}) starts at live");
        assert!(game.observers.is_empty());
        assert!(game.text.is_empty());
    }
}

#[test]
fn playing_finds_either_player_and_the_synchro_parent() {
    let mut lobby = Lobby::default();
    lobby.apply(
        OsMsg::MatchList(vec![
            MatchEntry::parse(".3 1500 ann 1500 bea 8 U"),
            MatchEntry::parse(".7.0 1500 cid 1500 dan s8 U"),
        ]),
        Instant::now(),
    );
    assert_eq!(lobby.playing("bea"), Some(".3"));
    assert_eq!(lobby.playing("cid"), Some(".7"));
    assert_eq!(lobby.playing("eve"), None);
}

fn join(lobby: &mut Lobby, id: &str, now: Instant) {
    let record = Box::new(GameRecord::parse(SPEC_JOIN).unwrap());
    assert!(lobby.apply(OsMsg::Join { id: id.into(), record }, now).is_empty());
}

#[test]
fn several_watches_coexist_and_match_notices_reach_both_halves() {
    let now = Instant::now();
    let mut lobby = joined(now);
    assert!(watch(&mut lobby, ".12").is_empty(), "already watching");
    lobby.apply(OsMsg::WatchAdd { name: "eve".into(), id: ".12".into() }, now);
    assert!(lobby.games[".12"].observers.contains("eve"));
    assert_eq!(watch(&mut lobby, ".20.1"), ["tell /os watch + .20"], "a board watches its match; nothing is unwatched");
    assert_eq!(lobby.focus.as_deref(), Some(".20.1"));
    assert!(lobby.games.contains_key(".12"), "other watched games stay");
    join(&mut lobby, ".20.0", Instant::now());
    assert_eq!(lobby.focus.as_deref(), Some(".20.1"), "the other half does not take the focus");
    join(&mut lobby, ".20.1", Instant::now());
    assert_eq!(lobby.focused().map(|g| g.id.as_str()), Some(".20.1"));
    assert_eq!(lobby.halves().count(), 2);
    lobby.apply(OsMsg::WatchAdd { name: "fay".into(), id: ".20".into() }, now);
    assert!(lobby.halves().all(|g| g.observers.contains("fay")));
    lobby.apply(OsMsg::MatchRemove { id: ".20".into(), finished: None }, now);
    assert!(lobby.halves().all(|g| g.phase != Phase::Live));
    assert_eq!(lobby.watched, [".12"], "the server's watch ended with the match");
    assert!(unwatch(&mut lobby, ".20.0").is_empty(), "nothing to tell the server");
    assert_eq!(lobby.games.keys().collect::<Vec<_>>(), [".12"]);
    assert_eq!(lobby.focus, None);
    assert_eq!(unwatch(&mut lobby, ".12"), ["tell /os watch - .12"]);
    assert!(lobby.watched.is_empty() && lobby.games.is_empty());
}

#[test]
fn watching_synchro_parent_focuses_an_existing_board() {
    let mut views = GameViews::default();
    let mut lobby = joined(Instant::now());
    join(&mut lobby, ".20.1", Instant::now());
    join(&mut lobby, ".20.0", Instant::now());
    views.navigate(lobby.focused().unwrap(), Nav::First);
    lobby.view = ViewMode::Synchro;

    assert!(watch(&mut lobby, ".20").is_empty(), "the match is already subscribed");
    assert_eq!(lobby.view, ViewMode::Focus, "watching selects the focus view");
    assert_eq!(lobby.focused().map(|g| g.id.as_str()), Some(".20.0"));
    assert_eq!(lobby.focused().and_then(|g| views.get(g).cursor), None, "switching boards follows the live position");
    assert_eq!(lobby.watched, [".12", ".20"], "no duplicate subscription");
    assert_eq!(lobby.halves().count(), 2);

    views.navigate(lobby.focused().unwrap(), Nav::First);
    assert!(watch(&mut lobby, ".20").is_empty());
    assert_eq!(
        lobby.focused().and_then(|g| views.get(g).cursor),
        Some(0),
        "refocusing the same board keeps its history cursor"
    );
}

#[test]
fn a_finished_synchro_board_does_not_finish_the_whole_match() {
    let now = Instant::now();
    let mut lobby = Lobby::default();
    let record = GameRecord::parse(&SPEC_JOIN.replace("TY[8]", "TY[s8]")).unwrap();
    for id in [".20.0", ".20.1"] {
        lobby.apply(OsMsg::Join { id: id.into(), record: Box::new(record.clone()) }, now);
    }
    assert!(lobby.games.values().all(|g| !g.match_ended));

    lobby.apply(OsMsg::End { id: ".20.0".into(), outcome: Outcome::Aborted }, now);
    assert_eq!(lobby.games[".20.0"].phase, Phase::Over(Outcome::Aborted));
    assert_eq!(lobby.games[".20.1"].phase, Phase::Live);
    assert!(lobby.games.values().all(|g| !g.match_ended), "the finished half still belongs to the current match");

    lobby.apply(OsMsg::MatchRemove { id: ".20".into(), finished: None }, now);
    assert!(
        lobby.games.values().all(|g| g.match_ended),
        "the match removal confirms both halves, including the already ended one"
    );
    assert_eq!(lobby.games[".20.0"].phase, Phase::Over(Outcome::Aborted), "the board keeps its own result");
    assert_ne!(lobby.games[".20.1"].phase, Phase::Live);
}

#[test]
fn watching_synchro_parent_uses_the_only_received_board() {
    let mut lobby = joined(Instant::now());
    join(&mut lobby, ".20.1", Instant::now());
    assert!(watch(&mut lobby, ".20").is_empty());
    assert_eq!(lobby.focused().map(|g| g.id.as_str()), Some(".20.1"));
}

#[test]
fn synchro_view_uses_only_the_selected_match_halves() {
    let now = Instant::now();
    let mut lobby = joined(now);
    join(&mut lobby, ".20.0", now);
    join(&mut lobby, ".30.1", now);
    lobby.show(".20.0");

    assert!(lobby.has_synchro_focus(), "one numbered half identifies a synchro match");
    let [first, second] = lobby.synchro_halves();
    assert_eq!(first.map(|g| g.id.as_str()), Some(".20.0"));
    assert!(second.is_none(), "the second board may still be arriving");
    assert!(
        lobby.synchro_halves().into_iter().flatten().all(|g| match_of(&g.id) == ".20"),
        "another watched match is excluded"
    );

    lobby.show_view(ViewMode::Synchro);
    lobby.focus = Some(".20.0".into());
    assert_eq!(lobby.view, ViewMode::Synchro, "changing the selected half keeps the pair view");
    unwatch(&mut lobby, ".20.0");
    assert_eq!(lobby.view, ViewMode::Focus, "closing the selected pair returns to focus view");
}

#[test]
fn synchro_view_is_available_from_match_metadata_before_either_board_arrives() {
    let now = Instant::now();
    let mut lobby = Lobby::default();
    lobby.apply(OsMsg::MatchList(vec![MatchEntry::parse(".5 1720 pbot 1720 edax s8r10 U")]), now);
    watch(&mut lobby, ".5");
    assert!(lobby.has_synchro_focus());
    assert!(lobby.synchro_halves().into_iter().all(|g| g.is_none()));
}

#[test]
fn watching_unseen_synchro_board_waits_for_that_exact_board() {
    let mut lobby = joined(Instant::now());
    join(&mut lobby, ".20.0", Instant::now());
    assert!(watch(&mut lobby, ".20.1").is_empty());
    assert_eq!(lobby.focus.as_deref(), Some(".20.1"));
    assert!(lobby.focused().is_none(), "an explicit board must not redirect to its sibling");
    join(&mut lobby, ".20.0", Instant::now());
    assert_eq!(lobby.focus.as_deref(), Some(".20.1"));
    join(&mut lobby, ".20.1", Instant::now());
    assert_eq!(lobby.focused().map(|g| g.id.as_str()), Some(".20.1"));
}

#[test]
fn watching_parent_without_a_board_waits_for_its_join() {
    let mut lobby = joined(Instant::now());
    join(&mut lobby, ".20.0", Instant::now());
    assert_eq!(watch(&mut lobby, ".2"), ["tell /os watch + .2"]);
    assert_eq!(lobby.focus.as_deref(), Some(".2"), "match IDs must match exactly, not by prefix");
    assert!(lobby.focused().is_none());
    join(&mut lobby, ".2.1", Instant::now());
    assert_eq!(lobby.focused().map(|g| g.id.as_str()), Some(".2.1"));
}

#[test]
fn watching_existing_game_keeps_the_exact_id() {
    let mut views = GameViews::default();
    let mut lobby = joined(Instant::now());
    views.navigate(lobby.focused().unwrap(), Nav::First);
    assert!(watch(&mut lobby, ".12").is_empty());
    assert_eq!(lobby.focused().map(|g| g.id.as_str()), Some(".12"));
    assert_eq!(lobby.focused().and_then(|g| views.get(g).cursor), Some(0));
}

#[test]
fn late_join_after_stop_is_ignored_until_watched_again() {
    let now = Instant::now();
    let mut lobby = joined(now);
    assert_eq!(unwatch(&mut lobby, ".12"), ["tell /os watch - .12"]);
    let record = Box::new(GameRecord::parse(SPEC_JOIN).unwrap());
    assert_eq!(lobby.apply(OsMsg::Join { id: ".12".into(), record }, now), ["ignored join for stopped .12"]);
    assert!(lobby.watched.is_empty() && lobby.games.is_empty(), "the stopped watch stays stopped");
    assert_eq!(watch(&mut lobby, ".12"), ["tell /os watch + .12"]);
    join(&mut lobby, ".12", Instant::now());
    assert_eq!(lobby.focused().map(|g| g.id.as_str()), Some(".12"));
}

/// An accepted undo and a decided komi both resend `join` for a game already shown (Game.C undo, play).

#[test]
fn rejoin_keeps_observers_and_kibitz_and_clamps_the_cursor() {
    let mut views = GameViews::default();
    let now = Instant::now();
    let mut lobby = joined(now);
    lobby.apply(OsMsg::WatchAdd { name: "eve".into(), id: ".12".into() }, now);
    lobby.apply(OsMsg::GameText { id: ".12".into(), lines: vec!["eve: hi".into()] }, now);
    lobby.apply(OsMsg::Update { id: ".12".into(), token: tok("e6//7") }, now);
    views.navigate(lobby.focused().unwrap(), Nav::Prev);
    views.navigate(lobby.focused().unwrap(), Nav::Next);
    views.navigate(lobby.focused().unwrap(), Nav::Prev);
    assert_eq!(lobby.focused().and_then(|g| views.get(g).cursor), Some(2));
    views.get_mut(lobby.focused().unwrap()).flipped = true;
    views.get_mut(lobby.focused().unwrap()).hints = false;
    lobby.focus = Some(".13".into());
    let undone = GameRecord::parse(&SPEC_JOIN.replace("W[c5/-1.23/4.5]", "").replace("12:00:00", "12:00:10")).unwrap();
    lobby.apply(OsMsg::Join { id: ".12".into(), record: Box::new(undone) }, now);
    let g = &lobby.games[".12"];
    assert_eq!(g.moves().len(), 1, "the undo took two moves back");
    assert!(g.observers.contains("eve"));
    assert_eq!(g.text, ["eve: hi"]);
    assert_eq!(
        views.get(g),
        GameView { cursor: None, flipped: true, hints: false },
        "an unfocused board also clamps after an undo"
    );
    lobby.focus = Some(".12".into());
    assert_eq!(
        lobby.focused().and_then(|g| views.get(g).cursor),
        None,
        "a cursor past the shortened game follows live"
    );
}

#[test]
fn a_recycled_match_id_gets_a_new_key_even_when_its_dated_record_is_identical() {
    let now = Instant::now();
    for whole_match_ended in [false, true] {
        let mut lobby = Lobby::default();
        join(&mut lobby, ".20.0", now);
        let old_key = lobby.games[".20.0"].key;
        if whole_match_ended {
            lobby.apply(OsMsg::MatchRemove { id: ".20".into(), finished: None }, now);
        } else {
            lobby.apply(OsMsg::End { id: ".20.0".into(), outcome: Outcome::Aborted }, now);
        }
        join(&mut lobby, ".20.0", now);
        let game = &lobby.games[".20.0"];
        assert_eq!(
            game.key == old_key,
            !whole_match_ended,
            "a board ending preserves its match identity; a completed match cannot own a recycled ID"
        );
        assert!(!game.match_ended, "the received live join belongs to an ongoing match");
    }
}

#[test]
fn komi_color_assignment_keeps_the_watch_and_view() {
    let now = Instant::now();
    let mut record = GameRecord::parse(SPEC_JOIN).unwrap();
    record.game_type = "8k".into();
    record.moves.clear();
    let mut lobby = Lobby::default();
    lobby.apply(OsMsg::Join { id: ".12".into(), record: Box::new(record.clone()) }, now);
    let key = lobby.games[".12"].key;
    lobby.apply(OsMsg::WatchAdd { id: ".12".into(), name: "eve".into() }, now);
    lobby.apply(OsMsg::GameText { id: ".12".into(), lines: vec!["eve: hello".into()] }, now);
    std::mem::swap(&mut record.black, &mut record.white);
    record.komi = Some(3.0);
    record.moves.push(tok("d3/4/2"));
    lobby.apply(OsMsg::Join { id: ".12".into(), record: Box::new(record) }, now);
    let game = &lobby.games[".12"];
    assert_eq!(game.key, key);
    assert_eq!(game.black.name, "bob");
    assert!(game.observers.contains("eve"));
    assert_eq!(game.text, ["eve: hello"]);
}

#[test]
fn a_new_match_notice_retires_even_an_identical_predecessor() {
    for reconnect in [false, true] {
        let now = Instant::now();
        let mut lobby = joined(now);
        let key = lobby.games[".12"].key;
        lobby.apply(OsMsg::GameText { id: ".12".into(), lines: vec!["old match".into()] }, now);
        if reconnect {
            lobby.suspend();
        }
        apply(&mut lobby, vec!["/os: + match .12 1850 alice 1720 bob 8 U".into()]);
        join(&mut lobby, ".12", now);
        assert_ne!(lobby.games[".12"].key, key);
        assert!(lobby.games[".12"].text.is_empty());
    }
}

#[test]
fn scored_synchro_joins_keep_each_board_result_at_match_end() {
    let now = Instant::now();
    let record = GameRecord::parse("(;PB[alice]PW[bob]TY[s4]RE[+16]BO[4 *O-- **** **** **** *]B[c1//1];)").unwrap();
    let mut lobby = Lobby::default();
    lobby.apply(OsMsg::Join { id: ".20.0".into(), record: Box::new(record.clone()) }, now);
    let game = &lobby.games[".20.0"];
    assert!(matches!(game.phase, Phase::Over(Outcome::Score(ref r)) if r.score == 16.));
    assert!(!game.match_ended);
    assert!(!game.is_running(Color::Black, None) && !game.is_running(Color::White, None));
    let mut other = record;
    std::mem::swap(&mut other.black, &mut other.white);
    other.result = None;
    other.moves.clear();
    lobby.apply(OsMsg::Join { id: ".20.1".into(), record: Box::new(other) }, now);
    apply(&mut lobby, vec!["/os: update .20.1 c1//1".into()]);
    apply(&mut lobby, vec!["/os: end .20.1 ( alice vs. bob ) -16.00".into()]);
    apply(&mut lobby, vec!["/os: - match .20 1850 alice 1720 bob s4 U 0.00".into()]);
    for game in lobby.games.values() {
        assert_eq!(GameRecord::parse(&game.to_ggf()).unwrap().result, Some(16.));
        assert!(game.match_ended);
    }
}

#[test]
fn a_scored_synchro_join_can_receive_its_withheld_final_move() {
    let now = Instant::now();
    let record = GameRecord::parse("(;PB[alice]PW[bob]TY[s4]RE[+16]BO[4 *O-- **** **** **** *];)").unwrap();
    let mut lobby = Lobby::default();
    lobby.apply(OsMsg::Join { id: ".20.0".into(), record: Box::new(record.clone()) }, now);
    assert!(matches!(lobby.games[".20.0"].phase, Phase::Over(_)));
    assert!(
        !apply(&mut lobby, vec!["/os: update .20.0 a1//1".into()]).is_empty(),
        "invalid moves do not consume the withheld update"
    );
    assert!(apply(&mut lobby, vec!["/os: update .20.0 c1//1".into()]).is_empty());
    assert_eq!(lobby.games[".20.0"].board(None).count(), (15, 0));
    assert!(
        !apply(&mut lobby, vec!["/os: update .20.0 pa//1".into()]).is_empty(),
        "only the withheld move may arrive after a scored join"
    );
    let mut archived = Game::archived("archive .20.0".into(), record, now).unwrap();
    assert!(archived.play(tok("c1//1"), now).is_err());
}

#[test]
fn a_scored_join_with_a_missing_move_needs_confirmation_after_a_stop() {
    for (reconnect, final_update) in [(false, false), (false, true), (true, false), (true, true)] {
        let now = Instant::now();
        let record = GameRecord::parse("(;PB[alice]PW[bob]TY[s4]RE[+16]BO[4 *O-- **** **** **** *];)").unwrap();
        let mut lobby = Lobby::default();
        lobby.apply(OsMsg::Join { id: ".20.0".into(), record: Box::new(record.clone()) }, now);
        if reconnect {
            let key = lobby.games[".20.0"].key;
            lobby.suspend();
            assert!(!lobby.games.contains_key(".20.0"), "an incomplete board waits for a fresh join");
            lobby.apply(OsMsg::Join { id: ".20.0".into(), record: Box::new(record) }, now);
            assert_eq!(lobby.games[".20.0"].key, key);
        }
        retain_watches(&mut lobby, |_| false);
        if final_update {
            assert!(apply(&mut lobby, vec!["/os: update .20.0 c1//1".into()]).is_empty());
        }
        lobby.apply(OsMsg::MatchRemove { id: ".20".into(), finished: None }, now);
        assert_eq!(
            lobby.games.contains_key(".20.0"),
            final_update,
            "only a confirmed final move makes the board complete"
        );
    }
}

#[test]
fn observer_notices_before_join_reach_both_synchro_halves() {
    for ids in [&[".12"][..], &[".20.0", ".20.1"][..]] {
        let now = Instant::now();
        let mut lobby = Lobby::default();
        let m = match_of(ids[0]);
        watch(&mut lobby, m);
        assert!(
            apply(&mut lobby, vec![format!("/os: + eve watch {m}")]).is_empty(),
            "watch notices normally precede joins"
        );
        apply(&mut lobby, vec![format!("/os: + gone watch {m}")]);
        apply(&mut lobby, vec![format!("/os: - gone watch {m}")]);
        for id in ids {
            join(&mut lobby, id, now);
        }
        assert!(lobby.games.values().all(|g| g.observers == BTreeSet::from(["eve".into()])));
        lobby.suspend();
        apply(&mut lobby, vec![format!("/os: + current watch {m}")]);
        for id in ids {
            join(&mut lobby, id, now);
        }
        assert!(lobby.games.values().all(|g| g.observers == BTreeSet::from(["current".into()])));
    }
}

#[test]
fn observer_notices_before_join_expire_with_the_subscription_or_match() {
    for cleanup in ["stop", "retain stop", "missing", "end", "start"] {
        let now = Instant::now();
        let mut lobby = Lobby::default();
        watch(&mut lobby, ".20");
        lobby.apply(OsMsg::WatchAdd { id: ".20".into(), name: "gone".into() }, now);
        match cleanup {
            "stop" => {
                unwatch(&mut lobby, ".20");
            }
            "retain stop" => {
                retain_watches(&mut lobby, |_| false);
            }
            "missing" => {
                lobby.apply(OsMsg::WatchMissing { ids: vec![".20".into()] }, now);
            }
            "end" => {
                lobby.apply(OsMsg::MatchRemove { id: ".20".into(), finished: None }, now);
            }
            _ => {
                lobby.apply(OsMsg::MatchAdd(MatchEntry::parse(".20 1850 alice 1720 bob s8 U")), now);
            }
        }
        watch(&mut lobby, ".20");
        lobby.apply(OsMsg::WatchAdd { id: ".20".into(), name: "current".into() }, now);
        for id in [".20.0", ".20.1"] {
            join(&mut lobby, id, now);
            assert_eq!(lobby.games[id].observers, BTreeSet::from(["current".into()]), "{cleanup}");
        }
    }
}

#[test]
fn reconnect_restores_each_board_and_clamps_a_shorter_record() {
    let mut views = GameViews::default();
    let now = Instant::now();
    let mut lobby = Lobby::default();
    let view = GameView { cursor: Some(1), flipped: true, hints: false };
    for (id, saved_view) in [(".12", view), (".13", GameView { cursor: Some(9), ..view })] {
        let old = Game::from_record(id.into(), GameRecord::parse(SPEC_JOIN).unwrap(), now).unwrap();
        *views.get_mut(&old) = saved_view;
        lobby.restoring_games.insert(id.into(), old);
    }
    for id in [".12", ".13"] {
        let mut record = GameRecord::parse(SPEC_JOIN).unwrap();
        record.date = Some("2006.08.17_13:00:00.GMT".into());
        lobby.apply(OsMsg::Join { id: id.into(), record: Box::new(record) }, now);
    }
    assert_eq!(views.get(&lobby.games[".12"]), view);
    assert_eq!(views.get(&lobby.games[".13"]), GameView { cursor: None, ..view });
    assert!(lobby.restoring_games.is_empty());
}

#[test]
fn synchro_parent_collapses_only_when_its_boards_are_listed() {
    let mut lobby = Lobby::default();
    let list = [".39", ".39.0", ".39.1", ".4", ".40.0"].map(|id| MatchEntry::parse(&format!("{id} x")));
    lobby.apply(OsMsg::MatchList(list.to_vec()), Instant::now());
    let ids: Vec<&str> = lobby.visible_matches().map(|e| e.id()).collect();
    assert_eq!(ids, [".39.0", ".39.1", ".4", ".40.0"]);
}

#[test]
fn users_keep_match_settings_across_who_refreshes() {
    let now = Instant::now();
    let user = |name: &str, status| User { name: name.into(), status, rating: "1500".into(), playing: None };
    let mut lobby = Lobby::default();
    who(&mut lobby, RatingType::Eight);
    lobby.apply(
        OsMsg::Who {
            users: vec![user("zed", UserStatus::Open), user("amy", UserStatus::Busy), user("bob", UserStatus::Open)],
            lines: vec![],
        },
        now,
    );
    assert_eq!(
        lobby.users().iter().map(|u| u.name.as_str()).collect::<Vec<_>>(),
        ["bob", "zed", "amy"],
        "open first, then by name"
    );
    let settings = MatchSettings {
        login: "zed".into(),
        rated: Some(true),
        accept: Some("size == 8".into()),
        ..Default::default()
    };
    lobby.apply(
        OsMsg::Finger {
            settings: vec![
                settings.clone(),
                MatchSettings { login: "ghost".into(), rated: Some(false), ..Default::default() },
            ],
            missing: vec![],
        },
        now,
    );
    who(&mut lobby, RatingType::Eight);
    lobby.apply(
        OsMsg::Who { users: vec![user("zed", UserStatus::Busy), user("cat", UserStatus::Open)], lines: vec![] },
        now,
    );
    assert_eq!(lobby.match_settings["zed"], Some(settings));
    lobby.apply(OsMsg::Finger { settings: vec![], missing: vec!["ghost".into()] }, now);
    assert_eq!(lobby.match_settings["ghost"], None, "a missing response replaces cached settings");
}

#[test]
fn finished_matches_keep_the_latest_twenty_and_their_games() {
    let mut lobby = joined(Instant::now());
    for i in 1..=22 {
        let id = if i == 22 { ".12".to_string() } else { format!(".{}", i + 100) };
        let line = format!("/os: - match {id} 1500 a 1500 b 8 R +{i}.00  .{}", 200 + i);
        let crate::proto::ServerMsg::Os(m) = crate::proto::parse(vec![line]) else { panic!("not an /os message") };
        assert!(lobby.apply(m, Instant::now()).is_empty());
    }
    let ids: Vec<&str> = lobby.finished.iter().map(|f| f.report.summary.id.as_str()).collect();
    let want: Vec<String> = (103..=121).map(|i| format!(".{i}")).chain([".12".to_string()]).collect();
    assert_eq!(ids, want, "oldest dropped first");
    assert_eq!(lobby.finished.back().and_then(|f| f.report.archive.as_deref()), Some(".222"));
    assert!(matches!(&lobby.games[".12"].phase, Phase::Over(Outcome::Score(r)) if r.score == 22.));
    lobby.focus = None;
    lobby.show(".12");
    assert_eq!(lobby.focused().map(|g| g.id.as_str()), Some(".12"));
    assert!(lobby.watched.is_empty(), "showing a held game does not watch it");
}

/// fixtures/live-archive.txt: `look` for a single game, a synchro pair, an adjourned game and a miss.

#[test]
fn looked_up_games_replay_as_ended_games() {
    let mut views = GameViews::default();
    let now = Instant::now();
    let mut lobby = joined(now);
    let apply = |lobby: &mut Lobby, lines: &[&str]| {
        let crate::proto::ServerMsg::Os(m) = crate::proto::parse(lines.iter().map(|s| s.to_string()).collect()) else {
            panic!()
        };
        lobby.apply(m, now)
    };
    assert_eq!(look(&mut lobby, ".73"), ["tell /os look .73"]);
    assert!(look(&mut lobby, ".73").is_empty(), "already asked");
    assert_eq!(lobby.focus.as_deref(), Some("archive .73"));
    assert!(lobby.focused().is_none(), "waiting for the reply");
    assert_eq!(look(&mut lobby, ".72"), ["tell /os look .72"]);
    let fixture = include_str!("../../fixtures/live-archive.txt");
    let bodies: Vec<String> = fixture
        .lines()
        .zip(fixture.lines().skip(1))
        .filter(|(h, _)| *h == "/os: look")
        .map(|(_, b)| b[1..].to_string())
        .collect();
    assert!(apply(&mut lobby, &["/os: look", &bodies[0]]).is_empty());
    assert!(apply(&mut lobby, &["/os: look", &bodies[1]]).is_empty());
    let g = &lobby.games["archive .73"];
    assert_eq!(
        (g.source, g.moves().len(), g.board(None).count()),
        (GameSource::Archive, 64, (0, 63)),
        "a wipeout with one square left empty"
    );
    assert!(matches!(&g.phase, Phase::Over(Outcome::Score(r)) if r.player1 == "prated" && r.score == -64.));
    assert!(!g.is_running(Color::Black, None) && !g.is_running(Color::White, None), "no clock runs");
    assert_eq!(lobby.focused().map(|g| g.id.as_str()), Some("archive .72.0"), "the latest request has the focus");
    look(&mut lobby, ".73");
    assert_eq!(lobby.focused().map(|g| g.id.as_str()), Some("archive .73"));
    assert_eq!(look(&mut lobby, ".72"), Vec::<String>::new(), "loaded, so only shown");
    assert_eq!(
        lobby.focused().map(|g| g.id.as_str()),
        Some("archive .72.0"),
        "a synchro pair opens on its first board"
    );
    assert_eq!(lobby.halves().count(), 2);
    assert!(lobby.has_synchro_focus());
    lobby.show_view(ViewMode::Synchro);
    assert!(
        lobby.synchro_halves().into_iter().flatten().all(|g| matches!(g.phase, Phase::Over(_))),
        "archived halves remain available together"
    );
    views.navigate(lobby.focused().unwrap(), Nav::First);
    assert_eq!(
        lobby.focused().and_then(|g| views.get(g).cursor),
        Some(0),
        "an archived game is browsed like an ended one"
    );
    assert!(unwatch(&mut lobby, "archive .72.0").is_empty(), "closing tells the server nothing");
    assert!(!lobby.games.keys().any(|g| g.starts_with("archive .72")) && lobby.games.contains_key("archive .73"));
    assert!(lobby.watched.iter().all(|w| !w.starts_with("archive")));
    assert_eq!(look(&mut lobby, ".17"), ["tell /os look .17"]);
    assert_eq!(
        apply(&mut lobby, &["/os: ERR no archived match '.17' found."]),
        [".17 is not in the archive; only rated matches are kept"]
    );
    assert_eq!(lobby.focus, None);
    assert_eq!(apply(&mut lobby, &["/os: look", &bodies[2]]), ["look reply that was not asked for"]);
    look(&mut lobby, ".5");
    let notes = apply(
        &mut lobby,
        &["/os: look", "1 (;BO[8 -------- -------- -------- ---O*--- ---*O--- -------- -------- -------- *]B[a1];)"],
    );
    assert_eq!(notes.len(), 1, "an unplayable record is reported");
    assert_eq!(lobby.focus, None, "and the view does not wait for it");
    assert_eq!(
        lines(super::games_of("prated").into()),
        ["finger prated", "tell /os finger prated", "tell /os history prated", "tell /os stored prated"]
    );
}

#[test]
fn a_stopped_watch_does_not_pass_its_frozen_board_off_as_finished() {
    let now = Instant::now();
    let mut lobby = joined(now);
    assert_eq!(retain_watches(&mut lobby, |_| false), ["tell /os watch - .12"]);
    assert!(lobby.games.contains_key(".12"), "kept for switching back");
    lobby.apply(OsMsg::MatchRemove { id: ".12".into(), finished: None }, now);
    assert!(lobby.games.is_empty(), "the archive holds the whole game");
}

#[test]
fn rewatching_without_a_new_join_still_drops_the_frozen_board() {
    let now = Instant::now();
    let mut lobby = joined(now);
    lobby.games.get_mut(".12").unwrap().observers.insert("gone".into());
    retain_watches(&mut lobby, |_| false);
    assert!(lobby.games[".12"].observers.is_empty(), "who left meanwhile is never announced");
    assert_eq!(watch(&mut lobby, ".12"), ["tell /os watch + .12"]);
    lobby.apply(OsMsg::MatchRemove { id: ".12".into(), finished: None }, now);
    assert!(lobby.games.is_empty(), "asking again is not the same as being resent the game");

    let mut lobby = joined(now);
    retain_watches(&mut lobby, |_| false);
    watch(&mut lobby, ".12");
    join(&mut lobby, ".12", now);
    lobby.apply(OsMsg::MatchRemove { id: ".12".into(), finished: None }, now);
    assert!(lobby.games.contains_key(".12"), "a resent game is whole");
}

#[test]
fn a_synchro_match_missing_one_board_opens_from_the_archive() {
    let now = Instant::now();
    let mut lobby = Lobby::default();
    join(&mut lobby, ".20.0", now);
    join(&mut lobby, ".20.1", now);
    lobby.apply(OsMsg::End { id: ".20.0".into(), outcome: Outcome::Aborted }, now);
    retain_watches(&mut lobby, |_| false);
    lobby.apply(OsMsg::MatchRemove { id: ".20".into(), finished: None }, now);
    assert!(lobby.games.is_empty(), "the ended board alone does not make the match held");
}

#[test]
fn a_stopped_watch_keeps_synchro_endings_already_in_flight() {
    let now = Instant::now();
    let mut assembler = crate::net::Assembler::default();
    let mut lobby = Lobby::default();
    let mut stopped = false;
    for line in include_str!("../../fixtures/live-synchro.txt").lines() {
        if let Some(crate::proto::ServerMsg::Os(msg)) = assembler.feed(line.to_string()).unwrap().map(proto::parse) {
            if matches!(&msg, OsMsg::End { .. }) && !stopped {
                assert_eq!(lobby.games.len(), 2);
                assert_eq!(retain_watches(&mut lobby, |_| false), ["tell /os watch - .5"]);
                stopped = true;
            }
            if matches!(&msg, OsMsg::MatchRemove { .. }) {
                assert!(lobby.games.values().all(|g| matches!(g.phase, Phase::Over(_))));
            }
            lobby.apply(msg, now);
        }
    }
    assert!(stopped);
    assert!(lobby.finished.back().unwrap().report.archive.is_none(), "an unrated match cannot be retrieved later");
    assert_eq!(lobby.games.len(), 2, "both complete boards were received through their end notices");
}

#[test]
fn a_stopped_watch_ignores_late_observers_until_rewatch() {
    let mut views = GameViews::default();
    let now = Instant::now();
    for ids in [&[".12"][..], &[".20.0", ".20.1"][..]] {
        let mut lobby = Lobby::default();
        let m = match_of(ids[0]);
        let view = GameView { cursor: Some(1), flipped: true, hints: false };
        for id in ids {
            join(&mut lobby, id, now);
            *views.get_mut(&lobby.games[*id]) = view;
        }
        retain_watches(&mut lobby, |_| false);
        // Sent before the server handled our stop; this observer leaves while unsubscribed.
        lobby.apply(OsMsg::WatchAdd { id: m.into(), name: "gone".into() }, now);
        watch(&mut lobby, m);
        // The server announces current observers before resending the boards.
        lobby.apply(OsMsg::WatchAdd { id: m.into(), name: "current".into() }, now);
        for id in ids {
            join(&mut lobby, id, now);
            assert_eq!(lobby.games[*id].observers, BTreeSet::from(["current".to_string()]));
            assert_eq!(views.get(&lobby.games[*id]), view);
        }
        lobby.apply(OsMsg::WatchRemove { id: m.into(), name: "current".into() }, now);
        assert!(lobby.games.values().all(|g| g.observers.is_empty()));
    }
}

#[test]
fn a_lost_who_reply_expires_instead_of_blocking_or_shifting_later_ones() {
    let mut lobby = Lobby::default();
    who(&mut lobby, RatingType::Eight);
    assert!(lobby.show_users(RatingType::Eight, Instant::now()), "still on its way");
    assert!(
        !lobby.show_users(RatingType::Eight, Instant::now() + Duration::from_secs(6)),
        "asked again after the cooldown"
    );
}

#[test]
fn a_late_who_reply_still_answers_its_own_request() {
    let mut lobby = Lobby::default();
    who(&mut lobby, RatingType::Eight);
    lobby.who_asked[0].at -= Duration::from_secs(60);
    who(&mut lobby, RatingType::EightRandom);
    lobby.apply(OsMsg::Who { users: Vec::new(), lines: Vec::new() }, Instant::now());
    assert!(
        lobby.users_cache.contains_key("8") && !lobby.users_cache.contains_key("8r"),
        "the late reply is the 8 list"
    );
}

#[test]
fn who_and_top_replies_take_the_type_they_were_asked_for() {
    let user = |name: &str| User { name: name.into(), status: UserStatus::Open, rating: "1500".into(), playing: None };
    let names = |lobby: &Lobby| lobby.users().iter().map(|u| u.name.clone()).collect::<Vec<_>>();
    let mut lobby = Lobby::default();
    assert_eq!(who(&mut lobby, RatingType::EightRandom), "tell /os who 8r a");
    assert_eq!(who(&mut lobby, RatingType::Eight), "tell /os who 8 a");
    lobby.apply(OsMsg::Who { users: vec![user("random")], lines: vec![] }, Instant::now());
    assert!(names(&lobby).is_empty(), "the first reply answers the first request, not the shown type");
    lobby.apply(OsMsg::Who { users: vec![user("eight")], lines: vec![] }, Instant::now());
    assert_eq!(names(&lobby), ["eight"]);
    lobby.apply(OsMsg::Who { users: vec![user("unasked")], lines: vec![] }, Instant::now());
    assert_eq!(names(&lobby), ["eight"], "an unasked reply cannot overwrite a known rating pool");
    assert!(lobby.show_users(RatingType::EightRandom, Instant::now()));
    assert_eq!(names(&lobby), ["random"]);
    assert_eq!(top(&mut lobby, RatingType::EightRandom), "tell /os top 8r 24");
    let rank = Rank { place: 1, name: "edax".into(), rating: 2000., stdev: 50., ascore: 1., win: 1, draw: 0, loss: 0 };
    lobby.apply(OsMsg::Top { ranks: vec![rank.clone()], lines: vec![] }, Instant::now());
    assert_eq!((lobby.ranks_type, lobby.ranks()), (RatingType::EightRandom, &[rank.clone()][..]));
    top(&mut lobby, RatingType::Eight);
    assert!(lobby.show_ranks(RatingType::Eight, Instant::now()), "a pending request does not need resending");
    lobby.apply(OsMsg::Top { ranks: Vec::new(), lines: Vec::new() }, Instant::now());
    assert!(lobby.show_ranks(RatingType::EightRandom, Instant::now()));
    assert_eq!(lobby.ranks(), [rank]);
    assert!(lobby.show_ranks(RatingType::Eight, Instant::now()));
    assert!(lobby.ranks().is_empty(), "an empty result is also cached");
}

#[test]
fn a_list_is_loading_only_between_its_request_and_its_reply() {
    let mut lobby = Lobby::default();
    assert!(!lobby.users_loading(Instant::now()) && !lobby.ranks_loading(Instant::now()), "nothing asked");
    who(&mut lobby, RatingType::Eight);
    top(&mut lobby, RatingType::Eight);
    assert!(lobby.users_loading(Instant::now()) && lobby.ranks_loading(Instant::now()));
    lobby.apply(OsMsg::Who { users: Vec::new(), lines: Vec::new() }, Instant::now());
    lobby.apply(OsMsg::Top { ranks: Vec::new(), lines: Vec::new() }, Instant::now());
    assert!(
        !lobby.users_loading(Instant::now()) && !lobby.ranks_loading(Instant::now()),
        "an empty reply is a loaded empty list"
    );
}

#[test]
fn own_kibitz_is_shown_once_and_sent_to_the_board_id() {
    let mut lobby = joined(Instant::now());
    assert_eq!(kibitz(&mut lobby, ".12", "me", "nice move", false), ["tell /os tell .12 nice move"]);
    assert_eq!(kibitz(&mut lobby, ".12", "me", "hush", true), ["tell /os tell o .12 hush"]);
    assert_eq!(lobby.games[".12"].text, ["me: nice move", "me (to observers): hush"]);
    let line = crate::chat::ChatLine { from: "eve".into(), lines: vec!["hi".into(), "there".into()] };
    assert!(lobby.apply(OsMsg::Kibitz { id: ".12".into(), line }, Instant::now()).is_empty());
    assert_eq!(lobby.games[".12"].text[2..], ["eve: hi", "there"]);
}

#[test]
fn match_notices_replace_entries_and_unknown_notices_are_reported() {
    let mut lobby = Lobby::default();
    let raw = |s: &str| MatchEntry::parse(s);
    lobby.apply(OsMsg::MatchList(vec![raw(".1 a"), raw(".2 b")]), Instant::now());
    lobby.apply(OsMsg::MatchAdd(raw(".3 c")), Instant::now());
    lobby.apply(OsMsg::MatchAdd(raw(".2 b2")), Instant::now());
    lobby.apply(OsMsg::MatchRemove { id: ".1".into(), finished: None }, Instant::now());
    assert_eq!(lobby.matches, [raw(".3 c"), raw(".2 b2")], "re-added entry replaces the old one");
    let notes = lobby.apply(OsMsg::Unknown(vec!["/os: huh".into()]), Instant::now());
    assert_eq!(notes, ["unknown /os message:", "/os: huh"]);
    let notes = lobby.apply(OsMsg::End { id: ".99".into(), outcome: Outcome::Aborted }, Instant::now());
    assert_eq!(notes, ["end notice for unknown game .99"]);
    let notes = lobby.apply(OsMsg::WatchAdd { name: "x".into(), id: ".99".into() }, Instant::now());
    assert!(notes.is_empty(), "observer state waits for a join");
}

fn apply(lobby: &mut Lobby, lines: Vec<String>) -> Vec<String> {
    let proto::ServerMsg::Os(message) = proto::parse(lines) else { panic!("not an os message") };
    lobby.apply(message, Instant::now())
}

#[test]
fn bad_archive_reply_must_not_relabel_the_next_record() {
    let mut lobby = Lobby::default();
    look(&mut lobby, ".101");
    look(&mut lobby, ".102");
    let errors = apply(&mut lobby, vec!["/os: look".into(), "1 (;PB[bad];)".into()]);
    assert!(!errors.is_empty());
    apply(&mut lobby, vec!["/os: look".into(), format!("1 {}", SPEC_JOIN.replace("PB[alice]", "PB[second]"))]);
    let loaded: Vec<_> = lobby.games.iter().map(|(id, game)| (id.clone(), game.black.name.clone())).collect();
    assert_eq!(
        loaded,
        vec![("archive .102".into(), "second".into())],
        "a failed .101 reply must not consume the later .102 record"
    );
}

#[test]
fn console_lists_and_rejected_queries_do_not_shift_gui_replies() {
    let mut lobby = Lobby::default();
    for command in ["who", "who bogus", "who 8 invalid", "top", "top bogus", "top 8 -1", "top 8 x", "top 8 1 2 3"] {
        lobby.sent(&proto::tell(command), CommandSource::Console);
    }
    lobby.sent("tell /os who 6 a\ntell /os top 8a", CommandSource::Console);
    who(&mut lobby, RatingType::EightRandom);
    top(&mut lobby, RatingType::Eight);
    assert_eq!(lobby.who_asked.len(), 2);
    assert_eq!(lobby.top_asked.len(), 2);
    apply(&mut lobby, vec!["/os: ERR rating type 'bogus' unknown.".into()]);
    apply(&mut lobby, vec!["/os: ERR who <type> [a] [-pts +pts].".into()]);
    apply(&mut lobby, vec!["/os: who 0".into()]);
    apply(&mut lobby, vec!["/os: top 0".into()]);
    assert!(lobby.users_cache.is_empty() && lobby.ranks_cache.is_empty());
    apply(&mut lobby, vec!["/os: who 0".into()]);
    apply(&mut lobby, vec!["/os: top 0".into()]);
    assert!(lobby.users_cache.contains_key("8r") && lobby.ranks_cache.contains_key("8"));
    assert!(lobby.who_asked.is_empty() && lobby.top_asked.is_empty());
}

#[test]
fn regression_filtered_console_lists_do_not_replace_gui_caches() {
    for (command, is_who) in
        [("who 8", true), ("who 8 a -1 1", true), ("top 8", false), ("top 8 1", false), ("top 8 2 24", false)]
    {
        let mut lobby = Lobby::default();
        let reply = |name: &str| {
            if is_who {
                OsMsg::Who {
                    users: vec![User {
                        name: name.into(),
                        status: UserStatus::Open,
                        rating: "1500".into(),
                        playing: None,
                    }],
                    lines: vec![],
                }
            } else {
                OsMsg::Top {
                    ranks: vec![Rank {
                        place: 1,
                        name: name.into(),
                        rating: 2000.,
                        stdev: 50.,
                        ascore: 1.,
                        win: 1,
                        draw: 0,
                        loss: 0,
                    }],
                    lines: vec![],
                }
            }
        };
        lobby.sent(&proto::tell(command), CommandSource::Console);
        let pending = if is_who {
            lobby.show_users(RatingType::Eight, Instant::now())
        } else {
            lobby.show_ranks(RatingType::Eight, Instant::now())
        };
        assert!(!pending, "{command} must not suppress a full GUI request");
        if is_who {
            who(&mut lobby, RatingType::Eight);
        } else {
            top(&mut lobby, RatingType::Eight);
        }
        lobby.apply(reply("filtered"), Instant::now());
        assert!(lobby.users().is_empty() && lobby.ranks().is_empty(), "{command} must consume only its own reply");
        lobby.apply(reply("full"), Instant::now());
        lobby.sent(&proto::tell(command), CommandSource::Console);
        lobby.apply(reply("filtered again"), Instant::now());
        let name = if is_who { &lobby.users()[0].name } else { &lobby.ranks()[0].name };
        assert_eq!(name, "full", "{command} must not replace a loaded GUI list");
    }
}

#[test]
fn regression_console_watches_share_the_gui_lifecycle() {
    let now = Instant::now();
    for ids in [&[".12"][..], &[".20.0", ".20.1"][..]] {
        let mut lobby = Lobby::default();
        let mut views = GameViews::default();
        let m = match_of(ids[0]);
        let view = GameView { cursor: Some(1), flipped: true, hints: false };
        for id in ids {
            join(&mut lobby, id, now);
            *views.get_mut(&lobby.games[*id]) = view;
        }
        retain_watches(&mut lobby, |_| false);
        lobby.sent(&format!("tell /os watch + {}", ids[0]), CommandSource::Console);
        assert!(lobby.is_watched(m), "a console rewatch must be tracked before the join");
        lobby.apply(OsMsg::WatchAdd { id: m.into(), name: "current".into() }, now);
        for id in ids {
            let record = GameRecord::parse(&SPEC_JOIN.replace(";)", "B[e6//5];)")).unwrap();
            assert!(lobby.apply(OsMsg::Join { id: (*id).into(), record: Box::new(record) }, now).is_empty());
            assert_eq!(lobby.games[*id].moves().len(), 3, "a console rewatch must accept the fresh record");
            assert!(lobby.games[*id].observers.contains("current"));
            assert_eq!(views.get(&lobby.games[*id]), view);
        }
        for command in [
            format!("tell /os watch + {}", ids[0]),
            "tell /os watch +".into(),
            "tell /os watch -".into(),
            "tell /os watch + *".into(),
        ] {
            lobby.sent(&command, CommandSource::Console);
            assert_eq!(
                lobby.watched,
                [m],
                "a duplicate watch, notification toggle or invalid target must not change subscriptions"
            );
        }
        let stop = if ids.len() == 1 { format!("tell /os watch - {}", ids[0]) } else { "tell /os watch %".into() };
        lobby.sent(&stop, CommandSource::Console);
        assert!(!lobby.is_watched(m));
        assert!(lobby.games.is_empty(), "a console stop closes the boards it stops");
        assert_eq!(
            lines(lobby.request_watch(m)),
            [format!("tell /os watch + {m}")],
            "a GUI rewatch must resubscribe after a console stop"
        );
    }
}

#[test]
fn regression_bad_list_headers_consume_only_their_own_requests() {
    for (command, is_who) in [("who", true), ("top", false)] {
        for header in [format!("/os: {command}"), format!("/os: {command} broken")] {
            let mut lobby = Lobby::default();
            lobby.sent(
                &proto::tell(&format!("{command} 8 {}", if is_who { "a" } else { "24" })),
                CommandSource::Console,
            );
            lobby.sent(
                &proto::tell(&format!("{command} 8r {}", if is_who { "a" } else { "24" })),
                CommandSource::Console,
            );
            assert!(!apply(&mut lobby, vec![header.clone()]).is_empty(), "{header} must be reported");
            assert!(
                lobby.users_cache.is_empty() && lobby.ranks_cache.is_empty(),
                "a malformed reply must not populate a cache"
            );
            apply(&mut lobby, vec![format!("/os: {command} 0")]);
            let cache = if is_who {
                lobby.users_cache.keys().copied().collect::<Vec<_>>()
            } else {
                lobby.ranks_cache.keys().copied().collect::<Vec<_>>()
            };
            assert_eq!(cache, ["8r"], "{header} must not shift the later 8r reply");
            assert!(lobby.who_asked.is_empty() && lobby.top_asked.is_empty());
        }
    }
}

#[test]
fn regression_a_rejected_watch_is_not_kept_or_resent() {
    let mut lobby = Lobby::default();
    lobby.sent("tell /os watch + .5", CommandSource::Console);
    assert_eq!(apply(&mut lobby, vec!["/os: watch + ERR not found: .5".into()]), ["not watching .5: no such match"]);
    assert!(lobby.watched.is_empty() && lobby.reconnect_commands().is_empty());
    assert_eq!(watch(&mut lobby, ".5"), ["tell /os watch + .5"], "a reused id can be watched");
}

#[test]
fn regression_stopping_watches_keeps_the_users_own_game() {
    let now = Instant::now();
    let mut lobby = Lobby { me: "alice".into(), ..Lobby::default() };
    join(&mut lobby, ".12", now);
    let other = GameRecord::parse(&SPEC_JOIN.replace("PB[alice]", "PB[carol]")).unwrap();
    lobby.apply(OsMsg::Join { id: ".13".into(), record: Box::new(other) }, now);
    for stop in ["tell /os watch %", "tell /os watch - .12"] {
        lobby.sent(stop, CommandSource::Console);
        assert_eq!(lobby.watched, [".12"], "{stop}: the server keeps sending the user's own game");
    }
    let record = GameRecord::parse(&SPEC_JOIN.replace(";)", "B[e6//5];)")).unwrap();
    assert!(lobby.apply(OsMsg::Join { id: ".12".into(), record: Box::new(record) }, now).is_empty());
    assert_eq!(lobby.games[".12"].moves().len(), 3);
}

#[test]
fn regression_recycled_own_ids_can_stop_watching() {
    for (reconnect, listed) in [(false, false), (false, true), (true, false), (true, true)] {
        for stop in ["tell /os watch - .12", "tell /os watch %"] {
            let now = Instant::now();
            let mut lobby = Lobby { me: "alice".into(), ..Lobby::default() };
            join(&mut lobby, ".12", now);
            if reconnect {
                lobby.suspend();
                lobby.os = OsState::Present;
            } else {
                lobby.apply(OsMsg::MatchRemove { id: ".12".into(), finished: None }, now);
            }
            if listed {
                apply(&mut lobby, vec!["/os: + match .12 1700 carol 1800 bob 8 R".into()]);
                assert_eq!(lobby.playing("carol"), Some(".12"));
            }
            lobby.sent("tell /os watch + .12", CommandSource::Console);
            lobby.sent(stop, CommandSource::Console);
            assert!(!lobby.is_watched(".12"), "{stop}, reconnect={reconnect}: the old owner cannot keep the new watch");
            let record = SPEC_JOIN.replace("PB[alice]", "PB[carol]");
            let reply = vec!["/os: join .12".into(), record];
            assert_eq!(apply(&mut lobby, reply.clone()), ["ignored join for stopped .12"]);
            assert_eq!(watch(&mut lobby, ".12"), ["tell /os watch + .12"]);
            assert!(apply(&mut lobby, reply).is_empty());
            assert_eq!(lobby.games[".12"].black.name, "carol");
        }
    }
}

#[test]
fn regression_own_join_is_kept_when_console_stops_before_the_board_arrives() {
    for listed in [false, true] {
        let now = Instant::now();
        let mut lobby = Lobby { me: "alice".into(), ..Lobby::default() };
        if listed {
            apply(&mut lobby, vec!["/os: + match .12 1700 alice 1800 bob 8 R".into()]);
        }
        lobby.sent("tell /os watch + .12", CommandSource::Console);
        lobby.sent("tell /os watch %", CommandSource::Console);
        assert!(apply(&mut lobby, vec!["/os: join .12".into(), SPEC_JOIN.into()]).is_empty());
        assert!(lobby.is_watched(".12") && lobby.games.contains_key(".12"));
        lobby.apply(OsMsg::WatchAdd { id: ".12".into(), name: "observer".into() }, now);
        assert!(
            lobby.games[".12"].observers.contains("observer"),
            "players keep getting the notices of a closed own match"
        );
        assert!(retain_watches(&mut lobby, |_| false).is_empty());
    }
}

#[test]
fn regression_a_watch_stopped_or_gone_during_reconnect_does_not_leave_boards_waiting() {
    let now = Instant::now();
    for console in ["tell /os watch - .20", "tell /os watch %"] {
        let mut lobby = Lobby::default();
        join(&mut lobby, ".20.0", now);
        join(&mut lobby, ".20.1", now);
        lobby.suspend();
        lobby.os = OsState::Present;
        lobby.sent(console, CommandSource::Console);
        assert!(
            lobby.restoring_games.is_empty() && lobby.games.is_empty() && !lobby.is_watched(".20"),
            "{console}: the stopped match loses its parked boards"
        );
        assert_eq!(lobby.focus, None, "{console}: no board will arrive for the shown match");
    }
    let mut lobby = Lobby::default();
    join(&mut lobby, ".20.0", now);
    lobby.suspend();
    apply(&mut lobby, vec!["/os: watch + ERR not found: .20".into()]);
    assert!(lobby.restoring_games.is_empty() && lobby.watched.is_empty());
}

#[test]
fn regression_console_lists_are_shown_in_the_log() {
    let mut lobby = Lobby::default();
    lobby.sent(&proto::tell("who 8"), CommandSource::Console);
    let who = vec!["/os: who 1".to_string(), "ann + 1500 .3".to_string()];
    assert_eq!(apply(&mut lobby, who.clone()), who);
    lobby.sent(&proto::tell("top 8 50"), CommandSource::Console);
    let top = vec!["/os: top 1".to_string(), "1 edax 2037.8@290.2= 00:01:06+@290.2 +1.0 1 0 0".to_string()];
    assert_eq!(apply(&mut lobby, top.clone()), top);
    assert!(lobby.users().is_empty() && lobby.ranks().is_empty());
}

#[test]
fn synchro_archive_load_must_be_atomic() {
    let mut lobby = Lobby::default();
    look(&mut lobby, ".101");
    let good = SPEC_JOIN.replace("TY[8]", "TY[s8]");
    let bad = good.replace("W[c5/-1.23/4.5]", "W[a1/-1.23/4.5]");
    let errors = apply(&mut lobby, vec!["/os: look".into(), format!("2 {good} {bad}")]);
    assert!(!errors.is_empty(), "the illegal second board must be detected");
    let blocked = lobby.request_look(".101").is_empty();
    assert!(
        lobby.games.is_empty() && !blocked,
        "partial boards: {:?}; retry blocked: {blocked}",
        lobby.games.keys().collect::<Vec<_>>()
    );
}

#[test]
fn regression_boards_of_a_match_stopped_around_a_reconnect_do_not_take_over_its_recycled_id() {
    for stopped_before_the_loss in [false, true] {
        let now = Instant::now();
        let mut lobby = Lobby::default();
        join(&mut lobby, ".5.0", now);
        join(&mut lobby, ".5.1", now);
        join(&mut lobby, ".4", now);
        lobby.show(".4");
        if stopped_before_the_loss {
            retain_watches(&mut lobby, |id| id == ".4");
            lobby.suspend();
        } else {
            lobby.suspend();
            retain_watches(&mut lobby, |id| id == ".4");
        }
        lobby.os = OsState::Present;
        // .5 ended during the outage and GGS reused its number for a normal match.
        apply(&mut lobby, vec!["/os: + match .5 1500 carol 1500 dave 8 R".into()]);
        watch(&mut lobby, ".5");
        assert!(
            lobby.focused().is_none(),
            "stopped before the loss: {stopped_before_the_loss}; an old board of .5 stands in for the new match"
        );
        let current = SPEC_JOIN.replace("PB[alice]", "PB[carol]").replace("PW[bob]", "PW[dave]");
        assert!(apply(&mut lobby, vec![format!("/os: join .5 {current}")]).is_empty());
        assert_eq!(lobby.focused().map(|g| g.black.name.as_str()), Some("carol"));
        assert!(
            !lobby.has_synchro_focus(),
            "stopped before the loss: {stopped_before_the_loss}; the old synchro halves are gone"
        );
        apply(&mut lobby, vec!["/os: - match .5 1500 carol 1500 dave 8 R +2.00 .80".into()]);
        assert_eq!(
            lobby.games.get(".5").map(|g| g.black.name.as_str()),
            Some("carol"),
            "stopped before the loss: {stopped_before_the_loss}; the complete new match is kept"
        );
        assert!(lobby.finished.back().unwrap().is_held(&lobby));
    }
}

#[test]
fn regression_stop_closes_the_users_own_game_for_good() {
    let now = Instant::now();
    let mut lobby = Lobby { me: "alice".into(), ..Lobby::default() };
    join(&mut lobby, ".12", now);
    assert!(unwatch(&mut lobby, ".12").is_empty(), "the server sends players their game whatever `watch` says");
    assert_eq!(lobby.focus, None);
    assert!(
        lobby.apply(OsMsg::Update { id: ".12".into(), token: tok("e6//7") }, now).is_empty(),
        "moves keep reaching the closed game"
    );
    let undone = GameRecord::parse(&SPEC_JOIN.replace("W[c5/-1.23/4.5]", "")).unwrap();
    assert!(lobby.apply(OsMsg::Join { id: ".12".into(), record: Box::new(undone) }, now).is_empty());
    assert_eq!(lobby.focus, None, "an accepted undo resends the join, which must not reopen the game");
    assert!(watch(&mut lobby, ".12").is_empty(), "reopening needs no subscription");
    assert_eq!(lobby.focused().map(|g| g.moves().len()), Some(1));
}

#[test]
fn regression_a_console_stop_closes_the_board_it_stops() {
    for stop in ["tell /os watch - .12", "tell /os watch %"] {
        let now = Instant::now();
        let mut lobby = joined(now);
        lobby.sent(stop, CommandSource::Console);
        assert_eq!(lobby.focus, None, "{stop}: nothing arrives for the shown board any more");
        assert!(lobby.games.is_empty() && !lobby.is_watched(".12"), "{stop}");
        lobby.apply(OsMsg::MatchRemove { id: ".12".into(), finished: None }, now);
        assert_eq!(lobby.focus, None, "{stop}");
    }
}

#[test]
fn regression_a_match_gone_during_reconnect_releases_the_view() {
    let now = Instant::now();
    let mut lobby = Lobby::default();
    join(&mut lobby, ".20.0", now);
    join(&mut lobby, ".20.1", now);
    lobby.show_view(ViewMode::Synchro);
    lobby.suspend();
    assert_eq!(apply(&mut lobby, vec!["/os: watch + ERR not found: .20".into()]), ["not watching .20: no such match"]);
    assert_eq!(
        (lobby.focus.as_deref(), lobby.view),
        (None, ViewMode::Focus),
        "no board will arrive for a match that is gone"
    );
}

#[test]
fn regression_commands_replayed_after_a_reconnect_login_do_not_cancel_its_watches() {
    let now = Instant::now();
    let mut lobby = joined(now);
    join(&mut lobby, ".20", now);
    lobby.os = OsState::Present;
    lobby.suspend();
    // Commands after login go out before /os is announced again; the server holds no watches yet.
    lobby.sent("tell /os watch %", CommandSource::Console);
    assert_eq!(lines(lobby.reconnect_commands()), ["tell /os watch + .12", "tell /os watch + .20"]);
}

#[test]
fn regression_requests_sent_before_os_is_announced_after_a_reconnect_take_their_replies() {
    let mut lobby = Lobby { os: OsState::Present, ..Lobby::default() };
    lobby.suspend();
    // A commands-after-login line: /os kept running while the connection was down.
    look(&mut lobby, ".73");
    lobby.os = OsState::Present;
    look(&mut lobby, ".74");
    apply(&mut lobby, vec!["/os: look".into(), format!("1 {SPEC_JOIN}")]);
    assert!(lobby.games.contains_key("archive .73") && !lobby.games.contains_key("archive .74"));
}

#[test]
fn regression_aliased_console_lines_register_when_their_ack_shows_the_tell() {
    let now = Instant::now();
    let mut lobby = joined(now);
    lobby.sent("t /os who 8 a", CommandSource::Console);
    assert!(lobby.who_asked.is_empty(), "what `t` expands to depends on the server's aliases");
    ack(&mut lobby, "tell /os who 8 a");
    let command = who(&mut lobby, RatingType::EightRandom);
    ack(&mut lobby, &command);
    assert_eq!(lobby.who_asked.len(), 2, "a tell registered when sent is not registered again by its ack");
    apply(&mut lobby, vec!["/os: who 1".into(), "zed + 1500@100".into()]);
    assert!(lobby.users_cache.contains_key("8") && !lobby.users_cache.contains_key("8r"));
    lobby.sent("t /os watch + .5\nt /os watch - .12", CommandSource::Console);
    ack(&mut lobby, "tell /os watch + .5");
    ack(&mut lobby, "tell /os watch - .12");
    assert_eq!(lobby.watched, [".5"]);
}

#[test]
fn regression_an_aliased_request_keeps_its_send_order_ahead_of_later_ones() {
    let mut lobby = Lobby::default();
    lobby.sent("t /os who 8 a", CommandSource::Console);
    let command = who(&mut lobby, RatingType::EightRandom);
    ack(&mut lobby, "tell /os who 8 a");
    ack(&mut lobby, &command);
    apply(&mut lobby, vec!["/os: who 1".into(), "zed + 1500@100".into()]);
    assert!(
        lobby.users_cache.contains_key("8") && !lobby.users_cache.contains_key("8r"),
        "the aliased who was sent first"
    );
}

#[test]
fn regression_a_stacked_alias_keeps_every_command_in_its_send_order() {
    let mut lobby = Lobby::default();
    lobby.sent("both", CommandSource::Console);
    lobby.sent("t /os who 8r a", CommandSource::Console);
    let command = top(&mut lobby, RatingType::Eight);
    ack(&mut lobby, "tell /os who 8 a");
    ack(&mut lobby, "tell /os top 8r 24");
    ack(&mut lobby, "tell /os who 8r a");
    ack(&mut lobby, &command);
    apply(&mut lobby, vec!["/os: who 1".into(), "zed + 1500@100".into()]);
    apply(&mut lobby, vec!["/os: top 1".into(), "1 zed 1500@100".into()]);
    assert!(lobby.users_cache.contains_key("8") && !lobby.users_cache.contains_key("8r"));
    assert!(
        lobby.ranks_cache.contains_key("8r") && !lobby.ranks_cache.contains_key("8"),
        "`both` sent its top before the GUI's"
    );
    assert_eq!((lobby.who_asked.len(), lobby.top_asked.len()), (1, 1), "each request is registered once");
}

#[test]
fn regression_an_aliased_stop_acked_after_a_later_watch_does_not_undo_it() {
    let now = Instant::now();
    let mut lobby = joined(now);
    lobby.sent("t /os watch - .12", CommandSource::Console);
    lobby.sent("tell /os watch + .12", CommandSource::App);
    ack(&mut lobby, "tell /os watch - .12");
    ack(&mut lobby, "tell /os watch + .12");
    join(&mut lobby, ".12", now);
    assert_eq!(lobby.watched, [".12"], "the server ran the stop first, then the watch");
    assert!(lobby.games.contains_key(".12"));
}

#[test]
fn regression_an_aliased_after_login_stop_does_not_cancel_the_watches_a_reconnect_restores() {
    let now = Instant::now();
    let mut lobby = joined(now);
    lobby.os = OsState::Present;
    lobby.connection_lost();
    lobby.sent("t /os watch %", CommandSource::Console);
    lobby.os = OsState::Present;
    let commands = lobby.reconnect_commands();
    execute(&mut lobby, commands);
    ack(&mut lobby, "tell /os watch %");
    assert_eq!(lobby.watched, [".12"]);
    assert_eq!(lines(lobby.reconnect_commands()), ["tell /os watch + .12"]);
}

#[test]
fn regression_lines_sent_before_a_connection_loss_do_not_claim_later_acks() {
    let mut lobby = Lobby::default();
    lobby.sent("tell /os who 8 a", CommandSource::Console);
    lobby.connection_lost();
    lobby.os = OsState::Present;
    lobby.sent("t /os who 8 a", CommandSource::Console);
    ack(&mut lobby, "tell /os who 8 a");
    assert_eq!(lobby.who_asked.len(), 1, "the ack is the aliased line's, sent on this connection");
}

fn ack(lobby: &mut Lobby, ran: &str) {
    lobby.acked(&format!("{ran} => {ran}"));
}

#[test]
fn regression_an_arrow_in_a_sent_line_does_not_split_its_ack() {
    let mut lobby = Lobby::default();
    let line = "tell bob Example => tell /os look .73";
    lobby.sent(line, CommandSource::Console);
    ack(&mut lobby, line);
    lobby.sent("al", CommandSource::Console);
    ack(&mut lobby, line);
    assert!(lobby.looks.is_empty(), "neither ack ran a look");
}

#[test]
fn regression_an_alias_ack_does_not_take_a_later_line_with_the_same_text() {
    let mut lobby = Lobby::default();
    lobby.sent("m", CommandSource::Console);
    look(&mut lobby, ".73");
    lobby.sent("tell /os match", CommandSource::Console);
    look(&mut lobby, ".74");
    for ran in ["tell /os match", "tell /os look .73", "tell /os match", "tell /os look .74"] {
        ack(&mut lobby, ran);
    }
    apply(&mut lobby, vec!["/os: look".into(), format!("1 {SPEC_JOIN}")]);
    assert!(lobby.games.contains_key("archive .73"));
    assert_eq!(lobby.looks.iter().map(|(_, key)| key.as_str()).collect::<Vec<_>>(), [".74"]);
}

#[test]
fn regression_a_variable_line_registers_its_expansion_once() {
    let mut lobby = Lobby::default();
    lobby.sent("tell /os look $archive", CommandSource::Console);
    lobby.acked("tell /os look $archive => tell /os look .73");
    assert_eq!(lobby.looks.iter().map(|(_, key)| key.as_str()).collect::<Vec<_>>(), [".73"]);
}

#[test]
fn regression_acks_of_lines_dropped_from_a_full_queue_do_not_register_again() {
    let mut lobby = Lobby::default();
    let keys: Vec<String> = (0..1100).map(|i| format!(".{i}")).collect();
    for key in &keys {
        look(&mut lobby, key);
    }
    for key in &keys {
        ack(&mut lobby, &proto::tell(&format!("look {key}")));
    }
    assert_eq!(lobby.looks.len(), keys.len());
    lobby.sent("t /os look .9999", CommandSource::Console);
    ack(&mut lobby, "tell /os look .9999");
    assert_eq!(lobby.looks.len(), keys.len() + 1, "acks line up again from the first own ack of a line still queued");
}

#[test]
fn regression_a_recycled_id_drops_the_earlier_match_of_the_other_shape() {
    let now = Instant::now();
    let mut lobby = joined(now);
    lobby.apply(OsMsg::End { id: ".12".into(), outcome: Outcome::Aborted }, now);
    join(&mut lobby, ".12.0", now);
    assert!(!lobby.games.contains_key(".12"));
    lobby.show(".12");
    assert_eq!(lobby.focused().map(|g| g.id.as_str()), Some(".12.0"));
}

#[test]
fn regression_a_parked_own_record_does_not_make_a_recycled_id_unstoppable() {
    for stop in ["tell /os watch - .12", "tell /os watch %"] {
        let now = Instant::now();
        let mut lobby = Lobby { me: "alice".into(), ..Lobby::default() };
        join(&mut lobby, ".12", now);
        lobby.os = OsState::Present;
        lobby.suspend();
        // /os is back but its match list has not arrived; .12 may belong to someone else by now.
        lobby.os = OsState::Present;
        lobby.sent(stop, CommandSource::Console);
        assert!(
            !lobby.is_watched(".12"),
            "{stop}: only the listing or a board of this login shows that .12 is still alice's"
        );
        let other = SPEC_JOIN.replace("PB[alice]", "PB[carol]");
        assert_eq!(
            apply(&mut lobby, vec![format!("/os: join .12 {other}")]),
            ["ignored join for stopped .12"],
            "{stop}"
        );
    }
}

#[test]
fn regression_console_list_replies_are_logged_as_the_server_sent_them() {
    let who = [
        "/os: who     3      change:    win    draw    loss         match(es)",
        "/os      - 1720.0@350.0 ->  +324.4    +0.0  -324.4 @290.2 ",
        "parch    - 1720.0@350.0 ",
        "edax     + 2072.3@278.0 ->  +649.7  +253.8  -142.0 @302.8",
    ];
    let top = [
        "/os: top       1720.0@352.7      Inactive       AScore    Win   Draw   Loss 2",
        "   1 edax     2072.7@278.0=     00:00:11+@278.0 +64.0      2      0      0",
        "   2 prated   1367.3@278.0=     00:00:11+@278.0 -64.0      0      0      2",
    ];
    for (command, reply) in [("who 8r a", &who[..]), ("top 8r 24", &top[..])] {
        let mut lobby = Lobby::default();
        lobby.sent(&proto::tell(command), CommandSource::Console);
        let lines: Vec<String> = reply.iter().map(|l| l.to_string()).collect();
        assert_eq!(apply(&mut lobby, lines.clone()), lines, "{command}: statuses, counts and columns stay as sent");
    }
}

#[test]
fn regression_console_words_are_split_like_ggs_splits_them() {
    let now = Instant::now();
    let mut lobby = joined(now);
    for line in ["tell /os watch -\u{a0}.12", "tell /os watch\u{3000}%"] {
        lobby.sent(line, CommandSource::Console);
    }
    assert_eq!(lobby.watched, [".12"], "GGS splits on ASCII spaces only, so it rejected both lines");
    lobby.sent("tell /os\twatch - .12", CommandSource::Console);
    assert_eq!(lobby.watched, [".12"], "`tell` takes `/os\\twatch` as the recipient, which the server rejects");
    lobby.sent("\ttell  /os watch\t-\t.12", CommandSource::Console);
    assert!(!lobby.is_watched(".12"), "a tab separates the command and /os's words");
}
