use super::{Conn, Launch, Rail, Screen, Viewer, note};
use crate::{
    chat::{Chat, ChatKey, ChatLine, DEFAULT_CHANNEL},
    ggf::{GameRecord, tests::SPEC_JOIN},
    lobby::{Lobby, ViewMode},
    net,
    proto::OsMsg,
    ui,
};
use gpui_kit::{
    AppContext, Bounds, ClipboardItem, Entity, Focusable, InputEvent, Keystroke, MouseButton, MouseDownEvent, Pixels,
    ScrollDelta, ScrollWheelEvent, Task, TestAppContext, VisualTestContext, point, px, size,
};
use gpui_kit::{
    base::{Root, TextSelection},
    test::TestWindowExt,
};
use std::time::{Instant, UNIX_EPOCH};

const COPY_KEY: &str = if cfg!(target_os = "macos") { "cmd-c" } else { "ctrl-c" };
const SELECT_ALL_KEY: &str = if cfg!(target_os = "macos") { "cmd-a" } else { "ctrl-a" };

/// Render the real Viewer and Root without a TCP connection, timing or demo-feed races.
fn fixture(cx: &mut TestAppContext) -> (Entity<Viewer>, &mut VisualTestContext) {
    cx.update(gpui_kit::init);
    let mut viewer = None;
    let (_, cx) = cx.add_window_view(|window, cx| {
        let entity =
            cx.new(|cx| Viewer::new(Launch::Login, ui::ThemeChoice::Dark, Ok(crate::store::memory()), window, cx));
        entity.update(cx, |v, _| {
            let (tx, rx) = async_channel::bounded(1);
            drop(rx);
            let mut chat = Chat::new(None, "me".into());
            chat.add(
                ChatKey::Channel(DEFAULT_CHANNEL.into()),
                ChatLine { from: "bob".into(), lines: vec!["chat output".into()] },
                UNIX_EPOCH,
            );
            let mut lobby = Lobby::default();
            lobby.apply(
                OsMsg::Join { id: ".12".into(), record: Box::new(GameRecord::parse(SPEC_JOIN).unwrap()) },
                Instant::now(),
            );
            lobby.games.get_mut(".12").unwrap().text.push("kibitz output".into());
            v.session.screen = Screen::Connected(Conn {
                link: net::demo(tx),
                label: "test".into(),
                chat,
                params: None,
                attempt: 0,
                logged_in_at: None,
                _pump: Task::ready(()),
            });
            v.session.lobby = lobby;
            v.rail = Rail::Chat;
            v.show_log = true;
            note(&mut v.log, "log output");
        });
        viewer = Some(entity.clone());
        Root::new(entity, window, cx)
    });
    cx.simulate_resize(size(px(1240.), px(920.)));
    settle(cx);
    (viewer.unwrap(), cx)
}

fn settle(cx: &mut VisualTestContext) {
    for _ in 0..4 {
        cx.run_until_parked();
        cx.update(|window, cx| window.render_frame(cx));
    }
}

#[gpui_kit::test]
fn repeated_refreshes_and_user_pages_limit_server_requests(cx: &mut TestAppContext) {
    let (viewer, cx) = fixture(cx);
    cx.update(|_, cx| {
        viewer.update(cx, |v, cx| {
            v.rail = Rail::Matches;
            v.refresh_rail(false, cx);
            assert!(
                !v.log.iter().flat_map(|b| &b.lines).any(|line| line == "> tell /os match"),
                "entering Matches uses live notifications"
            );
            v.rail = Rail::Users;
            v.refresh_rail(true, cx);
            v.refresh_rail(true, cx);
            v.on_message(vec!["/os: who 1".into(), "alice + 1500@100".into()], cx);
            assert!(
                !v.log.iter().flat_map(|b| &b.lines).any(|line| line.starts_with("> tell /os finger")),
                "listing users never requests their details"
            );
            v.open_games("alice".into(), cx);
            v.open_games("alice".into(), cx);
            let sent = |v: &Viewer, cmd: &str| v.log.iter().flat_map(|b| &b.lines).filter(|line| *line == cmd).count();
            assert_eq!(sent(v, "> tell /os who 8 a"), 1);
            assert_eq!(sent(v, "> tell /os finger alice"), 1, "only opening the page requests details");
            assert_eq!(sent(v, "> finger alice"), 1);
            assert_eq!(sent(v, "> tell /os history alice"), 1);
            assert_eq!(sent(v, "> tell /os stored alice"), 1);
            assert!(!v.allow_read("who 8".into()));
            v.session.reads.sent.insert("who 8".into(), Instant::now() - std::time::Duration::from_secs(5));
            v.refresh_rail(true, cx);
            assert_eq!(sent(v, "> tell /os who 8 a"), 2);
            v.refresh_rail(false, cx);
            assert_eq!(sent(v, "> tell /os who 8 a"), 2, "tab navigation within the cooldown reuses the loaded list");
            v.session.reads.sent.insert("who 8".into(), Instant::now() - std::time::Duration::from_secs(5));
            v.refresh_rail(false, cx);
            assert_eq!(sent(v, "> tell /os who 8 a"), 3, "tab navigation asks for the loaded list afresh");
            v.rating_type = crate::proto::RatingType::EightRandom;
            v.refresh_rail(false, cx);
            v.refresh_rail(false, cx);
            assert_eq!(sent(v, "> tell /os who 8r a"), 1);
            if let Some((_, lobby)) = v.session.connected_mut() {
                lobby.profiles.insert("alice".into(), None);
                lobby.match_settings.insert("alice".into(), None);
                lobby.history.insert("alice".into(), Vec::new());
                lobby.stored.insert("alice".into(), Vec::new());
            }
            v.session.reads.sent.remove("games alice");
            v.open_games("alice".into(), cx);
            assert_eq!(sent(v, "> finger alice"), 2, "reopening a loaded page asks for it afresh");
            v.session.reads.sent.remove("games alice");
        })
    });
    settle(cx);
    let button = cx.debug_bounds("rating-type-8").unwrap();
    cx.simulate_click(button.center(), Default::default());
    settle(cx);
    cx.update(|_, cx| {
        let v = viewer.read(cx);
        assert_eq!(
            v.rating_type,
            crate::proto::RatingType::Eight,
            "switching back uses the cached type even within the cooldown"
        );
        let Some((_, lobby)) = v.session.connected() else { unreachable!() };
        assert_eq!(lobby.users_type, crate::proto::RatingType::Eight);
    });
    let button = cx.debug_bounds("refresh-games").unwrap();
    cx.simulate_click(button.center(), Default::default());
    cx.simulate_click(button.center(), Default::default());
    cx.update(|_, cx| {
        let v = viewer.read(cx);
        for cmd in ["> finger alice", "> tell /os finger alice", "> tell /os history alice", "> tell /os stored alice"]
        {
            assert_eq!(
                v.log.iter().flat_map(|b| &b.lines).filter(|line| *line == cmd).count(),
                3,
                "explicit page refresh fetches fresh data once: {cmd}"
            );
        }
    });
    cx.update(|_, cx| {
        viewer.update(cx, |v, cx| {
            v.on_message(vec![": + /os 1".into()], cx);
            v.on_message(vec![": + /os 1".into()], cx);
            assert_eq!(
                v.log.iter().flat_map(|b| &b.lines).filter(|line| *line == "> tell /os vt100 -").count(),
                1,
                "duplicate presence notices do not repeat setup"
            );
            v.on_message(vec![": - /os 1".into()], cx);
            v.on_message(vec![": + /os 1".into()], cx);
            v.refresh_rail(false, cx);
            assert_eq!(
                v.log.iter().flat_map(|b| &b.lines).filter(|line| *line == "> tell /os who 8 a").count(),
                4,
                "a service restart clears cached lists and cooldowns"
            );
        })
    });
}

#[gpui_kit::test]
fn rapid_main_switches_reuse_watches_until_the_cleanup_tick(cx: &mut TestAppContext) {
    let (viewer, cx) = fixture(cx);
    cx.update(|_, cx| {
        viewer.update(cx, |v, cx| {
            v.watch(".13", cx);
            let deadline = v.session.watch_cleanup_at;
            v.watch(".12", cx);
            assert_eq!(
                v.session.watch_cleanup_at, deadline,
                "continued switching cannot postpone cleanup indefinitely"
            );
            let Some((_, lobby)) = v.session.connected() else { unreachable!() };
            assert!(lobby.is_watched(".12") && lobby.is_watched(".13"));
            assert!(
                !v.log
                    .iter()
                    .flat_map(|b| &b.lines)
                    .any(|line| line == "> tell /os watch + .12" || line.starts_with("> tell /os watch -")),
                "switching back avoids a full join"
            );
            v.watch(".13", cx);
            v.session.watch_cleanup_at = Some(Instant::now() - std::time::Duration::from_secs(5));
        })
    });
    cx.cx.executor().advance_clock(std::time::Duration::from_secs(1));
    settle(cx);
    cx.update(|_, cx| {
        let v = viewer.read(cx);
        let Some((_, lobby)) = v.session.connected() else { unreachable!() };
        assert!(!lobby.is_watched(".12") && lobby.is_watched(".13"));
        assert!(lobby.games.contains_key(".12"));
        assert!(v.log.iter().flat_map(|b| &b.lines).any(|line| line == "> tell /os watch - .12"));
        assert!(v.session.watch_cleanup_at.is_none());
    });
}

#[gpui_kit::test]
fn inherited_tracking_joins_release_hidden_watches_after_a_fixed_grace(cx: &mut TestAppContext) {
    let (viewer, cx) = fixture(cx);
    cx.update(|_, cx| {
        viewer.update(cx, |v, cx| {
            assert!(v.session.watch_cleanup_at.is_none());
            v.on_message(vec![format!("/os: join .13 {SPEC_JOIN}")], cx);
            let deadline = v.session.watch_cleanup_at;
            assert!(deadline.is_some(), "a join without a focus switch still schedules cleanup");
            v.on_message(vec![format!("/os: join .14 {SPEC_JOIN}")], cx);
            assert_eq!(v.session.watch_cleanup_at, deadline, "more tracked matches do not postpone cleanup");
            assert_eq!(v.session.lobby.focus.as_deref(), Some(".12"));
            assert!(v.session.lobby.is_watched(".13") && v.session.lobby.is_watched(".14"));
            v.session.watch_cleanup_at = Some(Instant::now() - std::time::Duration::from_secs(5));
        })
    });
    cx.cx.executor().advance_clock(std::time::Duration::from_secs(1));
    settle(cx);
    cx.update(|_, cx| {
        let v = viewer.read(cx);
        assert!(v.session.lobby.is_watched(".12"));
        assert!(!v.session.lobby.is_watched(".13") && !v.session.lobby.is_watched(".14"));
        for id in [".13", ".14"] {
            let command = format!("> tell /os watch - {id}");
            assert_eq!(v.log.iter().flat_map(|b| &b.lines).filter(|line| *line == &command).count(), 1);
        }
        assert!(v.session.watch_cleanup_at.is_none());
    });
}

#[gpui_kit::test]
fn kibitz_cannot_be_sent_from_a_finished_recycled_or_unwatched_board(cx: &mut TestAppContext) {
    let (viewer, cx) = fixture(cx);
    cx.update(|_, cx| {
        viewer.update(cx, |v, cx| {
            assert!(v.send_kibitz("live message".into(), cx));
            v.session.lobby.apply(OsMsg::MatchRemove { id: ".12".into(), finished: None }, Instant::now());
            v.session.lobby.apply(
                OsMsg::MatchAdd(crate::proto::MatchEntry::parse(".12 1800 carol 1900 dave 8 U")),
                Instant::now(),
            );
            let old_text = v.session.lobby.games[".12"].text.clone();
            let old_history = v.kibitz_recall.lines.len();
            assert!(!v.send_kibitz("message intended for the old players".into(), cx));
            assert_eq!(v.session.lobby.games[".12"].text, old_text);
            assert_eq!(v.kibitz_recall.lines.len(), old_history);
            assert!(
                !v.log.iter().flat_map(|b| &b.lines).any(|line| line.contains("message intended for the old players"))
            );
        })
    });
    settle(cx);
    assert!(cx.debug_bounds("kibitz-input").is_none(), "a finished board cannot offer an input for a recycled live id");
    cx.update(|_, cx| {
        viewer.update(cx, |v, cx| {
            let mut record = GameRecord::parse(SPEC_JOIN).unwrap();
            record.black.name = "carol".into();
            record.white.name = "dave".into();
            v.session.lobby.apply(OsMsg::Join { id: ".12".into(), record: Box::new(record) }, Instant::now());
            assert!(v.send_kibitz("message for the new players".into(), cx));
            v.session.lobby.discard_watches(|_| false);
            assert!(!v.send_kibitz("message from an unwatched retained board".into(), cx));
        })
    });
    settle(cx);
    assert!(cx.debug_bounds("kibitz-input").is_none());
}

#[gpui_kit::test]
fn watches_follow_main_selection_and_child_window_lifetime(cx: &mut TestAppContext) {
    let (viewer, cx) = fixture(cx);
    cx.update(|_, cx| {
        viewer.update(cx, |v, cx| {
            v.watch_in_window(".12", cx);
        })
    });
    settle(cx);
    let mut child = child_context(&viewer, ".12", cx);
    cx.update(|_, cx| {
        viewer.update(cx, |v, cx| {
            v.watch(".13", cx);
            let Some((_, lobby)) = v.session.connected_mut() else { unreachable!() };
            assert!(lobby.is_watched(".12"), "the child keeps its subscription");
            assert!(lobby.is_watched(".13"), "the main selection stays subscribed while join is pending");
        })
    });
    child.update(|window, _| window.remove_window());
    settle(cx);
    cx.update(|_, cx| {
        viewer.update(cx, |v, cx| {
            let Some((_, lobby)) = v.session.connected() else { unreachable!() };
            assert!(!lobby.is_watched(".12"));
            assert!(lobby.games.contains_key(".12"), "stopping a watch keeps its browsing state");
            assert!(v.session.watch_cleanup_at.is_some(), "a closed window leaves the main switch's grace running");
            assert!(lobby.is_watched(".13"));
            assert!(v.log.iter().flat_map(|b| &b.lines).any(|line| line == "> tell /os watch - .12"));
            v.watch_in_window(".13", cx);
        })
    });
    settle(cx);
    let mut child = child_context(&viewer, ".13", cx);
    child.update(|window, _| window.remove_window());
    settle(cx);
    cx.update(|_, cx| {
        let Some((_, lobby)) = viewer.read(cx).session.connected() else { unreachable!() };
        assert!(lobby.is_watched(".13"), "closing a child preserves the main selection");
    });
    cx.update(|_, cx| viewer.update(cx, |v, cx| v.watch_in_window(".14", cx)));
    settle(cx);
    let mut child = child_context(&viewer, ".14", cx);
    cx.update(|_, cx| {
        viewer.update(cx, |v, cx| {
            v.session.lobby.suspend();
            v.session.screen = Screen::Reconnecting(super::Retry {
                params: net::LoginParams {
                    host: "example.test".into(),
                    port: 5000,
                    login: "me".into(),
                    password: String::new(),
                    keepalive: net::KEEPALIVE,
                },
                attempt: 1,
                chat: Chat::new(None, "me".into()),
                _timer: Task::ready(()),
            });
            cx.notify();
        })
    });
    child.update(|window, _| window.remove_window());
    settle(cx);
    cx.update(|_, cx| {
        let Screen::Reconnecting(_) = &viewer.read(cx).session.screen else { unreachable!() };
        assert_eq!(
            viewer.read(cx).session.lobby.watched,
            [".13"],
            "closing a child during reconnect prevents resubscribing it"
        );
    });
}

fn add_synchro_pair(viewer: &Entity<Viewer>, cx: &mut VisualTestContext) {
    cx.update(|_, cx| {
        viewer.update(cx, |v, cx| {
            let Some((_, lobby)) = v.session.connected_mut() else { unreachable!() };
            let mut first = GameRecord::parse(SPEC_JOIN).unwrap();
            first.game_type = "s8r10".into();
            let mut second = first.clone();
            std::mem::swap(&mut second.black, &mut second.white);
            let now = Instant::now();
            lobby.apply(OsMsg::MatchAdd(crate::proto::MatchEntry::parse(".20 1850 alice 1720 bob s8r10 U")), now);
            lobby.apply(OsMsg::Join { id: ".20.0".into(), record: Box::new(first) }, now);
            lobby.apply(OsMsg::Join { id: ".20.1".into(), record: Box::new(second) }, now);
            lobby.show(".20.0");
            v.show_log = false;
            cx.notify();
        })
    });
    settle(cx);
}

fn open_synchro(viewer: &Entity<Viewer>, cx: &mut VisualTestContext) {
    let tab = cx.debug_bounds("view-synchro-tab").expect("the selected synchro match adds its view tab");
    cx.simulate_click(tab.center(), Default::default());
    settle(cx);
    cx.update(|_, cx| {
        let Some((_, lobby)) = viewer.read(cx).session.connected() else { unreachable!() };
        assert_eq!(lobby.view, ViewMode::Synchro);
    });
}

#[gpui_kit::test]
fn first_evaluation_does_not_move_the_player_panel_or_clocks(cx: &mut TestAppContext) {
    let (viewer, cx) = fixture(cx);
    cx.update(|_, cx| {
        viewer.update(cx, |v, cx| {
            let Some((_, lobby)) = v.session.connected_mut() else { unreachable!() };
            let mut record = GameRecord::parse(SPEC_JOIN).unwrap();
            for token in &mut record.moves {
                token.eval = None;
            }
            lobby.apply(OsMsg::Join { id: ".12".into(), record: Box::new(record) }, Instant::now());
            cx.notify();
        })
    });
    settle(cx);
    let selectors = ["player-panel-.12", "player-clock-.12-black", "player-clock-.12-white"];
    let before = selectors.map(|selector| cx.debug_bounds(selector).unwrap());
    cx.update(|_, cx| {
        viewer.update(cx, |v, cx| {
            let Some((_, lobby)) = v.session.connected_mut() else { unreachable!() };
            lobby.apply(
                OsMsg::Update { id: ".12".into(), token: crate::othello::MoveToken::parse("e6/1.5/3").unwrap() },
                Instant::now(),
            );
            assert_eq!(lobby.focused().unwrap().last_eval(crate::othello::Color::Black, None), Some(1.5));
            cx.notify();
        })
    });
    settle(cx);
    for (selector, bounds) in selectors.into_iter().zip(before) {
        assert_eq!(
            cx.debug_bounds(selector).unwrap(),
            bounds,
            "{selector} stays fixed when the first evaluation arrives"
        );
    }
    let moves = cx.update(|_, cx| {
        viewer.update(cx, |v, cx| {
            let Some((_, lobby)) = v.session.connected_mut() else { unreachable!() };
            v.board.views.navigate(lobby.focused().unwrap(), crate::game::Nav::First);
            cx.notify();
            lobby.focused().unwrap().moves().len()
        })
    });
    settle(cx);
    assert!(moves > 0, "the fixture has moves to replay");
    // `debug_bounds` takes a static selector; the leak is one short string per test run.
    let counter = format!("move 0 / {moves}").leak();
    assert!(cx.debug_bounds(counter).is_some(), "the move counter follows the replay cursor");
    let history_panel = cx.debug_bounds(selectors[0]).unwrap();
    assert_eq!(history_panel.size.height, before[0].size.height);
    for (selector, bounds) in selectors.into_iter().zip(before).skip(1) {
        let clock = cx.debug_bounds(selector).unwrap();
        assert_eq!(clock.size.height, bounds.size.height);
        assert_eq!(
            clock.top() - history_panel.top(),
            bounds.top() - before[0].top(),
            "{selector} keeps its vertical position when browsing without evaluations; the header may resize the board width"
        );
    }
}

fn login_entry(login: &str) -> crate::history::Entry {
    crate::history::Entry { host: "example.test".into(), port: "5000".into(), login: login.into() }
}

fn show_login(viewer: &Entity<Viewer>, login: &str, cx: &mut VisualTestContext) {
    cx.update(|window, cx| {
        viewer.update(cx, |v, cx| {
            v.session.screen = Screen::Login;
            v.fill_login(login_entry(login), window, cx);
        })
    });
    settle(cx);
}

fn toggle_remember(cx: &mut VisualTestContext) {
    let bounds = cx.debug_bounds("remember-password").unwrap();
    cx.simulate_click(bounds.center(), Default::default());
    settle(cx);
}

fn save_login(viewer: &Entity<Viewer>, password: &str, cx: &mut VisualTestContext) {
    cx.update(|window, cx| {
        viewer.update(cx, |v, cx| {
            v.form.password.update(cx, |s, cx| s.set_value(password, window, cx));
            let params = net::LoginParams {
                host: super::text(&v.form.host, cx),
                port: 5000,
                login: super::text(&v.form.login, cx),
                password: password.into(),
                keepalive: net::KEEPALIVE,
            };
            v.save_login_password(&params, cx);
        })
    });
    settle(cx);
}

#[gpui_kit::test]
fn remember_password_is_opt_in_and_unchecking_forgets_only_this_login(cx: &mut TestAppContext) {
    let (viewer, cx) = fixture(cx);
    show_login(&viewer, "alice", cx);
    cx.update(|_, cx| assert!(!viewer.read(cx).form.remember_password));
    save_login(&viewer, "not-saved", cx);
    cx.update(|_, cx| assert!(viewer.read(cx).form.credentials.entries.borrow().is_empty()));
    toggle_remember(cx);
    save_login(&viewer, "alice-secret", cx);
    show_login(&viewer, "bob", cx);
    cx.update(|_, cx| {
        let v = viewer.read(cx);
        assert!(!v.form.remember_password);
        assert!(v.form.password.read(cx).value().is_empty());
    });
    toggle_remember(cx);
    save_login(&viewer, "bob-secret", cx);
    show_login(&viewer, "alice", cx);
    cx.update(|_, cx| {
        let v = viewer.read(cx);
        assert!(v.form.remember_password);
        assert_eq!(v.form.password.read(cx).value().as_str(), "alice-secret");
        assert!(!format!("{:?}", v.form.history).contains("secret"));
    });
    toggle_remember(cx);
    show_login(&viewer, "alice", cx);
    cx.update(|_, cx| {
        let v = viewer.read(cx);
        assert!(!v.form.remember_password);
        assert!(v.form.password.read(cx).value().is_empty());
        assert_eq!(v.form.credentials.entries.borrow().len(), 1);
    });
    show_login(&viewer, "bob", cx);
    cx.update(|_, cx| assert_eq!(viewer.read(cx).form.password.read(cx).value().as_str(), "bob-secret"));
}

#[gpui_kit::test]
fn credential_store_is_untouched_until_a_password_is_saved(cx: &mut TestAppContext) {
    let (viewer, cx) = fixture(cx);
    cx.update(|_, cx| viewer.read(cx).form.credentials.fail.set(true));
    show_login(&viewer, "alice", cx);
    save_login(&viewer, "not-saved", cx);
    toggle_remember(cx);
    toggle_remember(cx);
    cx.update(|_, cx| {
        let v = viewer.read(cx);
        assert_eq!(v.form.error, None, "no read");
        assert!(
            v.log.iter().flat_map(|b| &b.lines).all(|s| s != "Could not update saved password."),
            "no write or delete"
        );
    });
}

fn saved_marks(v: &Viewer) -> i64 {
    v.db.as_ref().map_or(0, |db| db.query_row("SELECT COUNT(*) FROM saved_passwords", [], |r| r.get(0)).unwrap_or(0))
}

#[gpui_kit::test]
fn a_mark_without_a_stored_password_is_dropped(cx: &mut TestAppContext) {
    let (viewer, cx) = fixture(cx);
    show_login(&viewer, "alice", cx);
    cx.update(|_, cx| assert!(viewer.read(cx).mark_saved(&viewer.read(cx).credential_key(cx).unwrap(), true)));
    show_login(&viewer, "alice", cx);
    cx.update(|_, cx| assert_eq!(saved_marks(viewer.read(cx)), 0));
}

#[gpui_kit::test]
fn saved_password_marks_persist_and_clear_after_forgetting(cx: &mut TestAppContext) {
    let (viewer, cx) = fixture(cx);
    show_login(&viewer, "alice", cx);
    toggle_remember(cx);
    save_login(&viewer, "alice-secret", cx);
    let saved = |cx: &mut VisualTestContext| cx.update(|_, cx| saved_marks(viewer.read(cx)));
    assert_eq!(saved(cx), 1);
    toggle_remember(cx);
    assert_eq!(saved(cx), 0);
    cx.update(|_, cx| assert!(viewer.read(cx).form.credentials.entries.borrow().is_empty()));
}

#[gpui_kit::test]
fn passwords_are_not_saved_while_their_mark_cannot_be_written(cx: &mut TestAppContext) {
    let (viewer, cx) = fixture(cx);
    let db = |sql: &str, cx: &mut VisualTestContext| {
        cx.update(|_, cx| viewer.read(cx).db.as_ref().unwrap().execute_batch(sql).unwrap())
    };
    show_login(&viewer, "alice", cx);
    toggle_remember(cx);
    db("DROP TABLE saved_passwords", cx);
    save_login(&viewer, "alice-secret", cx);
    cx.update(|_, cx| {
        let v = viewer.read(cx);
        assert!(v.form.credentials.entries.borrow().is_empty(), "no unmarked password");
        assert_eq!(saved_marks(v), 0);
    });
    db("CREATE TABLE saved_passwords(key TEXT PRIMARY KEY)", cx);
    save_login(&viewer, "alice-secret", cx);
    cx.update(|_, cx| {
        let v = viewer.read(cx);
        assert_eq!(saved_marks(v), 1, "retried after recovery");
        assert_eq!(v.form.credentials.entries.borrow().len(), 1);
    });
}

#[gpui_kit::test]
fn passwords_are_not_saved_without_a_database(cx: &mut TestAppContext) {
    let (viewer, cx) = fixture(cx);
    show_login(&viewer, "alice", cx);
    toggle_remember(cx);
    let db = cx.update(|_, cx| viewer.update(cx, |v, _| v.db.take()));
    save_login(&viewer, "alice-secret", cx);
    cx.update(|_, cx| {
        let v = viewer.read(cx);
        assert!(v.form.credentials.entries.borrow().is_empty(), "no unmarked password");
        assert_eq!(saved_marks(v), 0);
        assert!(v.log.iter().flat_map(|b| &b.lines).any(|s| s == "Could not update saved password."));
    });
    cx.update(|_, cx| viewer.update(cx, |v, _| v.db = db));
    save_login(&viewer, "alice-secret", cx);
    cx.update(|_, cx| {
        let v = viewer.read(cx);
        assert_eq!(saved_marks(v), 1);
        assert_eq!(v.form.credentials.entries.borrow().len(), 1);
    });
}

#[gpui_kit::test]
fn saved_password_reads_cannot_override_password_edits_or_checkbox_choices(cx: &mut TestAppContext) {
    let (viewer, cx) = fixture(cx);
    show_login(&viewer, "alice", cx);
    cx.update(|_, cx| assert!(viewer.read(cx).mark_saved(&viewer.read(cx).credential_key(cx).unwrap(), true)));
    for edit_password in [true, false] {
        let (tx, rx) = async_channel::bounded(1);
        cx.update(|window, cx| {
            viewer.update(cx, |v, cx| {
                *v.form.credentials.delayed_read.borrow_mut() = Some(rx);
                v.restore_password(window, cx);
            })
        });
        settle(cx);
        if edit_password {
            cx.update(|window, cx| {
                viewer.update(cx, |v, cx| {
                    v.form.password.update(cx, |s, cx| {
                        s.set_value("typed", window, cx);
                        cx.emit(super::InputEvent::Change);
                    });
                })
            });
            settle(cx);
            cx.update(|window, cx| {
                viewer.read(cx).form.password.clone().update(cx, |s, cx| s.set_value("", window, cx))
            });
        } else {
            toggle_remember(cx);
        }
        tx.try_send(Some(("alice".into(), b"old-secret".to_vec()))).unwrap();
        settle(cx);
        cx.update(|_, cx| {
            let v = viewer.read(cx);
            assert!(v.form.password.read(cx).value().is_empty());
            assert_eq!(v.form.remember_password, !edit_password);
        });
        if !edit_password {
            toggle_remember(cx);
        }
    }
}

#[gpui_kit::test]
fn password_mutations_and_restores_wait_for_earlier_saves_and_deletions(cx: &mut TestAppContext) {
    let (viewer, cx) = fixture(cx);
    show_login(&viewer, "alice", cx);
    toggle_remember(cx);
    cx.update(|_, cx| {
        let v = viewer.read(cx);
        v.form
            .credentials
            .entries
            .borrow_mut()
            .insert(v.credential_key(cx).unwrap(), ("alice".into(), b"old-secret".to_vec()));
    });
    let (tx, rx) = async_channel::bounded(1);
    cx.update(|_, cx| *viewer.read(cx).form.credentials.delayed_write.borrow_mut() = Some(rx));
    save_login(&viewer, "delayed-secret", cx);
    toggle_remember(cx);
    show_login(&viewer, "bob", cx);
    show_login(&viewer, "alice", cx);
    tx.try_send(()).unwrap();
    settle(cx);
    cx.update(|_, cx| {
        let v = viewer.read(cx);
        assert!(v.form.credentials.entries.borrow().is_empty());
        assert!(v.form.password.read(cx).value().is_empty());
        assert!(!v.form.remember_password);
    });
}

#[gpui_kit::test]
fn saved_password_identity_changes_clear_restored_but_keep_typed_password(cx: &mut TestAppContext) {
    let (viewer, cx) = fixture(cx);
    show_login(&viewer, "alice", cx);
    toggle_remember(cx);
    save_login(&viewer, "alice-secret", cx);
    show_login(&viewer, "alice", cx);
    cx.update(|window, cx| {
        viewer.read(cx).form.port.clone().update(cx, |s, cx| {
            s.set_value("5001", window, cx);
            cx.emit(super::InputEvent::Change);
        })
    });
    settle(cx);
    cx.update(|window, cx| {
        let v = viewer.read(cx);
        assert!(v.form.password.read(cx).value().is_empty());
        assert!(!v.form.remember_password);
        v.form.password.clone().update(cx, |s, cx| {
            s.set_value("typed", window, cx);
            cx.emit(super::InputEvent::Change);
        });
    });
    settle(cx);
    cx.update(|window, cx| {
        viewer.read(cx).form.login.clone().update(cx, |s, cx| {
            s.set_value("bob", window, cx);
            cx.emit(super::InputEvent::Change);
        })
    });
    settle(cx);
    cx.update(|_, cx| assert_eq!(viewer.read(cx).form.password.read(cx).value().as_str(), "typed"));
}

#[gpui_kit::test]
fn saved_password_failures_show_generic_errors_without_the_secret(cx: &mut TestAppContext) {
    let (viewer, cx) = fixture(cx);
    show_login(&viewer, "alice", cx);
    toggle_remember(cx);
    cx.update(|_, cx| viewer.read(cx).form.credentials.fail.set(true));
    save_login(&viewer, "private-secret", cx);
    toggle_remember(cx);
    cx.update(|_, cx| {
        let v = viewer.read(cx);
        assert!(v.log.iter().flat_map(|b| &b.lines).any(|s| s == "Could not update saved password."));
        assert!(v.log.iter().flat_map(|b| &b.lines).all(|s| !s.contains("private-secret")));
    });
}

fn chat_body_bounds(viewer: &Entity<Viewer>, i: usize, cx: &mut VisualTestContext) -> Bounds<Pixels> {
    cx.update(|_, cx| {
        let v = viewer.read(cx);
        v.chat_scroll.bounds_for_item(i * 2 + 1).expect("message body is laid out")
    })
}

fn chat_body_visible(v: &Viewer, i: usize) -> bool {
    v.chat_scroll.bounds_for_item(i * 2 + 1).is_some_and(|b| b.intersects(&v.chat_scroll.viewport_bounds()))
}

#[gpui_kit::test]
fn details_sidebar_only_appears_with_a_focused_board(cx: &mut TestAppContext) {
    let (viewer, cx) = fixture(cx);
    add_synchro_pair(&viewer, cx);
    let focus_center = cx.debug_bounds("center").unwrap();
    for (label, sheet, mode, focus, visible) in [
        ("Focus board", None, ViewMode::Focus, Some(".12"), true),
        ("Help", Some(super::Sheet::Help(None)), ViewMode::Focus, Some(".20.0"), false),
        ("History", Some(super::Sheet::Games("alice".into())), ViewMode::Synchro, Some(".20.0"), false),
        ("Synchro", None, ViewMode::Synchro, Some(".20.0"), false),
        ("No selection", None, ViewMode::Focus, None, false),
        ("Waiting for a board", None, ViewMode::Focus, Some(".404"), false),
        ("Back to Focus", None, ViewMode::Focus, Some(".12"), true),
    ] {
        cx.update(|_, cx| {
            viewer.update(cx, |v, cx| {
                v.sheet = sheet;
                let Some((_, lobby)) = v.session.connected_mut() else { unreachable!() };
                lobby.view = mode;
                lobby.focus = focus.map(str::to_string);
                cx.notify();
            })
        });
        settle(cx);
        assert_eq!(cx.debug_bounds("details").is_some(), visible, "{label} should only show details for a Focus board");
        let tabs_visible = label == "Synchro";
        assert_eq!(
            cx.debug_bounds("view-focus-tab").is_some(),
            tabs_visible,
            "{label} only offers a view choice when displaying a Synchro match"
        );
        assert_eq!(cx.debug_bounds("view-synchro-tab").is_some(), tabs_visible);
        assert!(cx.debug_bounds("view-grid-tab").is_none());
        if !visible {
            let center = cx.debug_bounds("center").unwrap();
            assert!(center.right() > focus_center.right(), "{label} should reclaim the sidebar space");
        }
    }
}

#[gpui_kit::test]
fn synchro_tab_shows_the_latest_pair_without_changing_focus_or_board_state(cx: &mut TestAppContext) {
    let (viewer, cx) = fixture(cx);
    assert!(cx.debug_bounds("view-synchro-tab").is_none(), "an ordinary match has no synchro tab");
    assert!(cx.debug_bounds("view-focus-tab").is_none(), "an ordinary match needs no view switcher");
    add_synchro_pair(&viewer, cx);
    let focus_tab = cx.debug_bounds("view-focus-tab").unwrap();
    let synchro_tab = cx.debug_bounds("view-synchro-tab").unwrap();
    assert!(focus_tab.left() < synchro_tab.left(), "Focus precedes Synchro");
    assert!(cx.debug_bounds("view-grid-tab").is_none());
    cx.update(|_, cx| {
        viewer.update(cx, |v, cx| {
            let Some((_, lobby)) = v.session.connected_mut() else { unreachable!() };
            lobby.apply(
                OsMsg::Update { id: ".20.0".into(), token: crate::othello::MoveToken::parse("e6//3").unwrap() },
                Instant::now(),
            );
            cx.notify();
        })
    });
    cx.update(|window, cx| {
        window.press("home", cx);
        window.press("f", cx);
    });
    settle(cx);
    open_synchro(&viewer, cx);

    assert!(cx.debug_bounds("synchro-board-.20.0").is_some());
    assert!(cx.debug_bounds("synchro-board-.20.1").is_some());
    assert!(cx.debug_bounds("synchro-board-.12").is_none(), "another watched game is not rendered in the pair");

    cx.update(|window, cx| {
        for key in ["left", "right", "home", "end", "f"] {
            window.press(key, cx);
        }
    });
    settle(cx);
    cx.update(|_, cx| {
        let v = viewer.read(cx);
        let Some((_, lobby)) = v.session.connected() else { unreachable!() };
        assert_eq!(
            (v.board.views.get(&lobby.games[".20.0"]).cursor, v.board.views.get(&lobby.games[".20.0"]).flipped),
            (Some(0), true),
            "Synchro keys preserve the Focus view state"
        );
        assert_eq!(
            (v.board.views.get(&lobby.games[".20.1"]).cursor, v.board.views.get(&lobby.games[".20.1"]).flipped),
            (None, false)
        );
        assert_eq!(lobby.view, ViewMode::Synchro);
    });
    assert!(
        cx.debug_bounds("player-elapsed-.20.0").is_some(),
        "Synchro shows the live position despite the Focus history cursor"
    );
    assert!(
        cx.debug_bounds("player-clock-.20.0-black").unwrap().left()
            < cx.debug_bounds("player-clock-.20.0-white").unwrap().left(),
        "Synchro keeps canonical orientation despite the Focus flip"
    );
    assert!(cx.debug_bounds("synchro-board-.20.0").is_some() && cx.debug_bounds("synchro-board-.20.1").is_some());

    cx.update(|window, cx| window.press("g", cx));
    settle(cx);
    cx.update(|_, cx| {
        let Some((_, lobby)) = viewer.read(cx).session.connected() else { unreachable!() };
        assert_eq!(lobby.view, ViewMode::Synchro, "G no longer changes the view");
    });

    let tab = cx.debug_bounds("view-focus-tab").unwrap();
    cx.simulate_click(tab.center(), Default::default());
    settle(cx);
    cx.update(|_, cx| {
        let v = viewer.read(cx);
        let Some((_, lobby)) = v.session.connected() else { unreachable!() };
        assert_eq!(lobby.view, ViewMode::Focus, "view-focus-tab maps to its named view when Synchro is present");
        assert_eq!(
            (v.board.views.get(&lobby.games[".20.0"]).cursor, v.board.views.get(&lobby.games[".20.0"]).flipped),
            (Some(0), true)
        );
    });
    assert!(cx.debug_bounds("player-elapsed-.20.0").is_none());
    assert!(
        cx.debug_bounds("player-clock-.20.0-white").unwrap().left()
            < cx.debug_bounds("player-clock-.20.0-black").unwrap().left()
    );
}

#[gpui_kit::test]
fn clicking_a_synchro_card_opens_that_board_in_focus(cx: &mut TestAppContext) {
    let (viewer, cx) = fixture(cx);
    add_synchro_pair(&viewer, cx);
    open_synchro(&viewer, cx);
    let second = cx.debug_bounds("synchro-card-.20.1").unwrap();
    cx.simulate_click(second.center(), Default::default());
    settle(cx);
    cx.update(|_, cx| {
        let Some((_, lobby)) = viewer.read(cx).session.connected() else { unreachable!() };
        assert_eq!(lobby.focus.as_deref(), Some(".20.1"));
        assert_eq!(lobby.view, ViewMode::Focus);
    });
    assert!(cx.debug_bounds("details").is_some());
}

#[gpui_kit::test]
fn synchro_boards_share_a_row_when_wide_and_stack_when_narrow(cx: &mut TestAppContext) {
    let (viewer, cx) = fixture(cx);
    add_synchro_pair(&viewer, cx);
    open_synchro(&viewer, cx);

    let center = cx.debug_bounds("center").unwrap();
    let first = cx.debug_bounds("synchro-card-.20.0").unwrap();
    let second = cx.debug_bounds("synchro-card-.20.1").unwrap();
    assert_eq!(first.top(), second.top(), "a wide center uses two columns");
    assert!(first.right() <= second.left());
    for selector in ["synchro-board-.20.0", "synchro-board-.20.1"] {
        let board = cx.debug_bounds(selector).unwrap();
        assert!(
            board.left() >= center.left() && board.right() <= center.right(),
            "{selector} {board:?}, center {center:?}"
        );
    }

    cx.simulate_resize(size(px(800.), px(920.)));
    settle(cx);
    let center = cx.debug_bounds("center").unwrap();
    let first = cx.debug_bounds("synchro-card-.20.0").unwrap();
    let second = cx.debug_bounds("synchro-card-.20.1").unwrap();
    assert!(second.top() >= first.bottom(), "a narrow center stacks the boards: {first:?}, {second:?}");
    for selector in ["synchro-board-.20.0", "synchro-board-.20.1"] {
        let board = cx.debug_bounds(selector).unwrap();
        assert!(
            board.left() >= center.left() && board.right() <= center.right(),
            "{selector} {board:?}, center {center:?}"
        );
    }
}

#[gpui_kit::test]
fn elapsed_updates_do_not_resize_boards_or_panels(cx: &mut TestAppContext) {
    let (viewer, cx) = fixture(cx);
    add_synchro_pair(&viewer, cx);
    for (mode, selectors) in [
        (
            ViewMode::Focus,
            vec!["player-panel-.20.0", "player-clock-.20.0-black", "player-clock-.20.0-white", "focus-board"],
        ),
        (
            ViewMode::Synchro,
            vec![
                "player-panel-.20.0",
                "player-clock-.20.0-black",
                "player-clock-.20.0-white",
                "synchro-card-.20.0",
                "synchro-board-.20.0",
                "synchro-card-.20.1",
                "synchro-board-.20.1",
            ],
        ),
    ] {
        cx.update(|_, cx| {
            viewer.update(cx, |v, cx| {
                let Some((_, lobby)) = v.session.connected_mut() else { unreachable!() };
                let mut record = GameRecord::parse(SPEC_JOIN).unwrap();
                record.game_type = "s8r10".into();
                lobby.apply(OsMsg::Join { id: ".20.0".into(), record: Box::new(record) }, Instant::now());
                lobby.show_view(mode);
                cx.notify();
            })
        });
        settle(cx);
        let before: Vec<_> = selectors
            .iter()
            .map(|selector| cx.debug_bounds(selector).unwrap_or_else(|| panic!("missing {selector}")))
            .collect();
        for token in ["e6//3", "f3//4"] {
            cx.update(|_, cx| {
                viewer.update(cx, |v, cx| {
                    let Some((_, lobby)) = v.session.connected_mut() else { unreachable!() };
                    assert!(
                        lobby
                            .apply(
                                OsMsg::Update {
                                    id: ".20.0".into(),
                                    token: crate::othello::MoveToken::parse(token).unwrap()
                                },
                                Instant::now()
                            )
                            .is_empty()
                    );
                    cx.notify();
                })
            });
            settle(cx);
            for (selector, bounds) in selectors.iter().zip(&before) {
                assert_eq!(
                    cx.debug_bounds(selector).unwrap(),
                    *bounds,
                    "{selector} stays fixed after elapsed starts or changes sides"
                );
            }
        }
    }
}

#[gpui_kit::test]
fn synchro_move_elapsed_appears_after_an_update_in_each_live_view(cx: &mut TestAppContext) {
    let (viewer, cx) = fixture(cx);
    add_synchro_pair(&viewer, cx);
    assert!(cx.debug_bounds("player-elapsed-.20.0").is_none(), "a join does not reveal when the current move began");

    cx.update(|_, cx| {
        viewer.update(cx, |v, cx| {
            let Some((_, lobby)) = v.session.connected_mut() else { unreachable!() };
            assert!(
                lobby
                    .apply(
                        OsMsg::Update { id: ".20.0".into(), token: crate::othello::MoveToken::parse("e6//3").unwrap() },
                        Instant::now()
                    )
                    .is_empty()
            );
            cx.notify();
        })
    });
    settle(cx);

    let panel = cx.debug_bounds("player-panel-.20.0").unwrap();
    let clock = cx.debug_bounds("player-clock-.20.0-white").unwrap();
    let elapsed = cx.debug_bounds("player-elapsed-.20.0").unwrap();
    let cell = cx.debug_bounds("player-cell-.20.0-white").unwrap();
    assert!(
        elapsed.right() <= cell.right() && elapsed.bottom() <= clock.top() && elapsed.bottom() > clock.top() - px(30.),
        "Focus elapsed {elapsed:?} shares the evaluation row just above the clock {clock:?} in {cell:?}"
    );
    assert!(elapsed.left() >= panel.left() && elapsed.right() <= panel.right() && elapsed.bottom() <= panel.bottom());

    open_synchro(&viewer, cx);
    let panel = cx.debug_bounds("player-panel-.20.0").unwrap();
    let elapsed = cx.debug_bounds("player-elapsed-.20.0").unwrap();
    assert!(elapsed.left() >= panel.left() && elapsed.right() <= panel.right() && elapsed.bottom() <= panel.bottom());
    assert!(
        cx.debug_bounds("player-elapsed-.20.1").is_none(),
        "the other half still has an unknown current-move start"
    );

    let tab = cx.debug_bounds("view-focus-tab").unwrap();
    cx.simulate_click(tab.center(), Default::default());
    settle(cx);
    cx.update(|window, cx| window.press("left", cx));
    settle(cx);
    assert!(cx.debug_bounds("player-elapsed-.20.0").is_none(), "browsing history hides the live elapsed time");
}

#[gpui_kit::test]
fn synchro_view_survives_reconnect_and_waits_for_each_board(cx: &mut TestAppContext) {
    let (viewer, cx) = fixture(cx);
    add_synchro_pair(&viewer, cx);
    open_synchro(&viewer, cx);
    let records = cx.update(|_, cx| {
        viewer.update(cx, |v, cx| {
            let Some((_, lobby)) = v.session.connected_mut() else { unreachable!() };
            lobby.focus = Some(".20.1".into());
            v.board.views.navigate(lobby.focused().unwrap(), crate::game::Nav::First);
            v.board.views.get_mut(lobby.focused().unwrap()).flipped = true;
            let records: Vec<_> = lobby.halves().map(|g| (g.id.clone(), g.to_ggf())).collect();
            lobby.suspend();
            v.on_message(vec![": + /os".into()], cx);
            cx.notify();
            records
        })
    });
    settle(cx);
    assert!(cx.debug_bounds("view-synchro-tab").is_some());
    assert!(cx.debug_bounds("synchro-waiting-0").is_some() && cx.debug_bounds("synchro-waiting-1").is_some());
    for (i, (id, record)) in records.into_iter().enumerate() {
        cx.update(|_, cx| {
            viewer.update(cx, |v, cx| {
                v.on_message(vec![format!("/os: join {id} {record}")], cx);
                cx.notify();
            })
        });
        settle(cx);
        assert!(cx.debug_bounds(["synchro-board-.20.0", "synchro-board-.20.1"][i]).is_some());
        if i == 0 {
            assert!(cx.debug_bounds("synchro-waiting-1").is_some());
        }
    }
    cx.update(|_, cx| {
        let v = viewer.read(cx);
        let Some((_, lobby)) = v.session.connected() else { unreachable!() };
        assert_eq!(lobby.view, ViewMode::Synchro);
        assert_eq!(lobby.focus.as_deref(), Some(".20.1"));
        assert_eq!(
            (v.board.views.get(&lobby.games[".20.1"]).cursor, v.board.views.get(&lobby.games[".20.1"]).flipped),
            (Some(0), true)
        );
        assert!(!lobby.restoring_games.contains_key(".20.0") && !lobby.restoring_games.contains_key(".20.1"));
    });
}

#[gpui_kit::test]
fn selecting_output_copies_it_after_an_input_was_focused(cx: &mut TestAppContext) {
    let (viewer, cx) = fixture(cx);
    for (i, expected) in ["chat output\n", "kibitz output", "log output"].into_iter().enumerate() {
        let chat_bounds = (i == 0).then(|| chat_body_bounds(&viewer, 0, cx));
        let (input, bounds) = cx.update(|window, cx| {
            let v = viewer.read(cx);
            let input = [&v.chat_input, &v.kibitz_input, &v.chat_input][i].clone();
            let output_id = match i {
                0 => ("chat-body", 0),
                1 => ("kibitz-line", 0),
                _ => ("log-block", v.log.front().unwrap().id),
            };
            input.update(cx, |s, cx| s.set_value("input selection", window, cx));
            let focus = input.read(cx).focus_handle(cx);
            window.click(("input", input.entity_id()), cx);
            window.press(SELECT_ALL_KEY, cx);
            assert!(focus.is_focused(window));
            cx.write_to_clipboard(ClipboardItem::new_string("untouched".into()));
            let bounds = chat_bounds.unwrap_or_else(|| window.find(output_id).bounds());
            (input, bounds)
        });
        let from = point(bounds.left() + px(1.), bounds.center().y);
        let to = point(bounds.right() - px(1.), bounds.center().y);
        cx.simulate_mouse_move(from, None, Default::default());
        cx.update(|window, cx| {
            window.dispatch_event(
                MouseDownEvent {
                    button: MouseButton::Left,
                    position: from,
                    modifiers: Default::default(),
                    click_count: 1,
                    first_mouse: false,
                }
                .to_platform_input(),
                cx,
            );
            assert!(
                !input.read(cx).focus_handle(cx).is_focused(window),
                "output takes focus when selection starts, before repainting"
            );
        });
        cx.simulate_mouse_move(to, Some(MouseButton::Left), Default::default());
        cx.simulate_mouse_up(to, MouseButton::Left, Default::default());
        cx.update(|window, cx| {
            window.draw(cx).clear(cx);
            assert_eq!(TextSelection::selected_text(window, cx), expected, "the output is highlighted");
            window.dispatch_keystroke(Keystroke::parse(COPY_KEY).unwrap(), cx);
            assert_eq!(
                cx.read_from_clipboard().and_then(|c| c.text()),
                Some(expected.trim_end_matches('\n').into()),
                "Copy must use the output with input {input:?}"
            );
            assert!(!input.read(cx).focus_handle(cx).is_focused(window));
        });
    }
}

#[gpui_kit::test]
fn board_controls_fit_the_constrained_center_after_a_resize(cx: &mut TestAppContext) {
    let (viewer, cx) = fixture(cx);
    cx.update(|_, cx| {
        viewer.update(cx, |v, cx| {
            v.show_log = false;
            cx.notify();
        })
    });
    // A tall window makes width, rather than height, limit the board size.
    for height in [920., 1400.] {
        cx.simulate_resize(size(px(960.), px(height)));
        settle(cx);
        let center = cx.debug_bounds("center").unwrap();
        assert_eq!(center.size.width, px(440.));
        for element in ["focus-board", "board-controls"] {
            let bounds = cx.debug_bounds(element).unwrap();
            assert!(bounds.left() >= center.left(), "{element} {bounds:?}, center {center:?}");
            assert!(bounds.right() <= center.right(), "{element} {bounds:?}, center {center:?}");
        }
    }
}

#[gpui_kit::test]
fn the_rail_tabs_stay_put_on_every_rail(cx: &mut TestAppContext) {
    let (viewer, cx) = fixture(cx);
    let tabs = |cx: &mut VisualTestContext| {
        ["rail-tab-0", "rail-tab-1", "rail-tab-2", "rail-tab-3", "rail-tab-4", "rail-refresh"]
            .map(|t| cx.debug_bounds(t).unwrap())
    };
    let mut seen = Vec::new();
    for rail in super::RAILS.map(|(r, _)| r) {
        cx.update(|_, cx| {
            viewer.update(cx, |v, cx| {
                v.rail = rail;
                cx.notify();
            })
        });
        settle(cx);
        seen.push(tabs(cx));
    }
    assert!(seen.windows(2).all(|w| w[0] == w[1]), "the tab row moved between rails: {seen:?}");
}

#[gpui_kit::test]
fn the_right_pane_stays_inside_an_800px_window(cx: &mut TestAppContext) {
    let (_, cx) = fixture(cx);
    for width in [900., 800., 760., 1240., 800.] {
        cx.simulate_resize(size(px(width), px(920.)));
        settle(cx);
        let details = cx.debug_bounds("details").unwrap();
        assert!(details.right() <= px(width), "right pane {details:?} exceeds the {width}px viewport");
        let center = cx.debug_bounds("center").unwrap();
        let board = cx.debug_bounds("focus-board").unwrap();
        assert!(board.left() >= center.left() && board.right() <= center.right(), "board {board:?}, center {center:?}");
    }
}

#[gpui_kit::test]
fn resized_panels_use_their_laid_out_space(cx: &mut TestAppContext) {
    let (viewer, cx) = fixture(cx);
    let before = cx.debug_bounds("center").unwrap();
    let from = point(before.left(), before.center().y);
    cx.update(|window, cx| window.drag(from, from + point(px(180.), px(0.)), cx));
    settle(cx);
    let narrower = cx.debug_bounds("center").unwrap();
    assert!(narrower.size.width < before.size.width, "the side divider was dragged");
    assert!(cx.update(|_, cx| viewer.read(cx).layout.left) > ui::LEFT_W, "the next launch keeps the width");

    let from = point(narrower.center().x, narrower.bottom());
    cx.update(|window, cx| window.drag(from, from - point(px(0.), px(120.)), cx));
    settle(cx);
    let shorter = cx.debug_bounds("center").unwrap();
    assert!(shorter.size.height < narrower.size.height, "the log divider was dragged");
    assert!(cx.update(|_, cx| viewer.read(cx).layout.log) > ui::LOG_H, "the next launch keeps the height");
    let board = cx.debug_bounds("focus-board").unwrap();
    assert!(
        board.right() <= shorter.right() && board.bottom() <= shorter.bottom(),
        "board {board:?}, center {shorter:?}"
    );

    // Saved side widths must shrink even when they originally exceeded their minima.
    cx.simulate_resize(size(px(800.), px(920.)));
    settle(cx);
    let details = cx.debug_bounds("details").unwrap();
    assert!(details.right() <= px(800.), "right pane {details:?} exceeds the viewport after dragging");
}

#[gpui_kit::test]
fn kibitz_input_stays_accessible_when_the_log_is_fully_expanded(cx: &mut TestAppContext) {
    let (viewer, cx) = fixture(cx);
    for viewport in [size(px(1240.), px(920.)), size(px(800.), px(480.))] {
        cx.simulate_resize(viewport);
        settle(cx);
        let center = cx.debug_bounds("center").unwrap();
        let from = point(center.center().x, center.bottom());
        cx.update(|window, cx| window.drag(from, point(from.x, px(0.)), cx));
        settle(cx);
        let details = cx.debug_bounds("details").unwrap();
        assert_eq!(details.size.height, px(240.), "the log reaches its allowed maximum height");
        assert!(details.bottom() < viewport.height, "the log still fits below the body");
        // Scroll the rail itself, away from its nested move and message lists.
        let position = point(details.left() + px(8.), details.center().y);
        cx.simulate_mouse_move(position, None, Default::default());
        cx.update(|window, cx| {
            window.dispatch_event(
                ScrollWheelEvent {
                    position,
                    delta: ScrollDelta::Pixels(point(px(0.), px(-400.))),
                    ..Default::default()
                }
                .to_platform_input(),
                cx,
            );
        });
        settle(cx);
        let controls = cx.debug_bounds("kibitz-controls").unwrap();
        assert!(
            controls.top() >= details.top() && controls.bottom() <= details.bottom(),
            "kibitz controls {controls:?} stay within {details:?}"
        );
        cx.update(|window, cx| {
            let input = viewer.read(cx).kibitz_input.clone();
            let bounds = window.find(("input", input.entity_id())).bounds();
            assert!(
                bounds.top() >= details.top() && bounds.bottom() <= details.bottom(),
                "kibitz input {bounds:?} is covered by the log below {details:?}"
            );
            window.click(("input", input.entity_id()), cx);
            assert!(input.read(cx).focus_handle(cx).is_focused(window), "the visible kibitz input receives clicks");
        });
    }
}

#[gpui_kit::test]
fn chat_selection_survives_scrolling_out_of_view(cx: &mut TestAppContext) {
    let (viewer, cx) = fixture(cx);
    cx.update(|_, cx| {
        viewer.update(cx, |v, cx| {
            let Screen::Connected(conn) = &mut v.session.screen else { unreachable!() };
            conn.chat.conversations[0].lines.clear();
            v.chat_shown.0 = None;
            for i in 0..80 {
                conn.chat.add(
                    ChatKey::Channel(DEFAULT_CHANNEL.into()),
                    ChatLine { from: "bob".into(), lines: vec![format!("message {i}")] },
                    UNIX_EPOCH,
                );
            }
            v.open_chat(ChatKey::Channel(DEFAULT_CHANNEL.into()), cx);
        })
    });
    settle(cx);
    let bounds = chat_body_bounds(&viewer, 79, cx);
    cx.update(|window, cx| {
        window.drag(
            point(bounds.left() + px(1.), bounds.center().y),
            point(bounds.right() - px(1.), bounds.center().y),
            cx,
        );
        assert_eq!(TextSelection::selected_text(window, cx), "message 79\n");
    });
    let position = cx.debug_bounds("rail-list").unwrap().center();
    cx.simulate_mouse_move(position, None, Default::default());
    cx.update(|window, cx| {
        window.dispatch_event(
            ScrollWheelEvent { position, delta: ScrollDelta::Pixels(point(px(0.), px(50_000.))), ..Default::default() }
                .to_platform_input(),
            cx,
        );
    });
    settle(cx);
    cx.update(|window, cx| {
        assert!(!chat_body_visible(viewer.read(cx), 79));
        assert_eq!(
            TextSelection::selected_text(window, cx),
            "message 79\n",
            "unmounting the selected message must preserve the selection"
        );
        window.press(COPY_KEY, cx);
        assert_eq!(cx.read_from_clipboard().and_then(|c| c.text()), Some("message 79".into()));
    });
    cx.update(|_, cx| {
        viewer.update(cx, |v, cx| {
            assert!(v.send_chat("message 80".into()));
            cx.notify();
        })
    });
    settle(cx);
    cx.update(|window, cx| {
        assert_eq!(
            TextSelection::selected_text(window, cx),
            "message 79\n",
            "appending preserves the selected endpoints"
        );
    });

    // Extend a drag across virtualized blocks, including all unmounted messages.
    cx.update(|_, cx| {
        viewer.read(cx).chat_scroll.scroll_to(gpui_kit::ListOffset { item_ix: 0, offset_in_item: px(0.) });
    });
    settle(cx);
    let first = chat_body_bounds(&viewer, 0, cx);
    let from = point(first.left() + px(1.), first.center().y);
    cx.simulate_mouse_move(from, None, Default::default());
    cx.update(|window, cx| {
        window.dispatch_event(
            MouseDownEvent {
                button: MouseButton::Left,
                position: from,
                modifiers: Default::default(),
                click_count: 1,
                first_mouse: false,
            }
            .to_platform_input(),
            cx,
        );
        viewer.read(cx).chat_scroll.scroll_to_end();
    });
    settle(cx);
    let last = chat_body_bounds(&viewer, 80, cx);
    let to = point(last.right() - px(1.), last.center().y);
    cx.simulate_mouse_move(to, Some(MouseButton::Left), Default::default());
    cx.simulate_mouse_up(to, MouseButton::Left, Default::default());
    cx.update(|window, cx| {
        let selected = TextSelection::selected_text(window, cx);
        assert!(selected.starts_with("message 0\n"), "{selected:?}");
        for i in 1..=80 {
            assert!(selected.contains(&format!("message {i}\n")), "offscreen message {i} is included");
        }
        window.press(COPY_KEY, cx);
        assert_eq!(cx.read_from_clipboard().and_then(|c| c.text()), Some(selected.trim_end_matches('\n').into()));
    });
}

#[gpui_kit::test]
fn chat_document_keeps_protocol_text_literal(cx: &mut TestAppContext) {
    let (viewer, cx) = fixture(cx);
    let body = "  <tag>& **literal** \\ [link](https://example.test)\n日本語\t  next\n\n\nlast";
    cx.update(|_, cx| {
        viewer.update(cx, |v, cx| {
            let Screen::Connected(conn) = &mut v.session.screen else { unreachable!() };
            conn.chat.add(
                ChatKey::Channel(DEFAULT_CHANNEL.into()),
                ChatLine { from: "a_b&<c>".into(), lines: body.split('\n').map(str::to_owned).collect() },
                UNIX_EPOCH,
            );
            cx.notify();
        })
    });
    settle(cx);
    let short = chat_body_bounds(&viewer, 0, cx);
    let multiline = chat_body_bounds(&viewer, 1, cx);
    assert!(multiline.size.height >= short.size.height * 4., "blank lines occupy visible space");
    cx.update(|window, cx| {
        let state = viewer.read(cx).chat_text.clone();
        assert!(
            state.read(cx).rendered_text().as_str().contains(body),
            "markup, whitespace and Unicode remain literal: {:?}",
            state.read(cx).rendered_text().as_str()
        );
        assert!(state.read(cx).rendered_text().as_str().contains("a_b&<c>"));
        state.update(cx, |state, cx| state.select_all(cx));
        assert!(state.read(cx).selected_text().contains(body), "copying preserves the original message");
        let focus = state.read(cx).focus_handle().clone();
        window.focus(&focus, cx);
        window.press(COPY_KEY, cx);
        assert!(
            cx.read_from_clipboard().and_then(|c| c.text()).is_some_and(|text| text.contains(body)),
            "Copy preserves consecutive blank lines"
        );
    });
}

#[gpui_kit::test]
fn offscreen_chat_messages_are_not_rendered_but_remain_scrollable(cx: &mut TestAppContext) {
    let (viewer, cx) = fixture(cx);
    cx.update(|_, cx| {
        viewer.update(cx, |v, cx| {
            let Screen::Connected(conn) = &mut v.session.screen else { unreachable!() };
            conn.chat.conversations[0].lines.clear();
            v.chat_shown.0 = None;
            for i in 0..500 {
                conn.chat.add(
                    ChatKey::Channel(DEFAULT_CHANNEL.into()),
                    ChatLine {
                        from: "bob".into(),
                        lines: if i % 5 == 0 {
                            vec![format!("message {i}"), "a second line".into(), "a third line".into()]
                        } else {
                            vec![format!("message {i}")]
                        },
                    },
                    UNIX_EPOCH,
                );
            }
            v.open_chat(ChatKey::Channel(DEFAULT_CHANNEL.into()), cx);
        })
    });
    let started = Instant::now();
    settle(cx);
    eprintln!("500-message chat settled in {:?}", started.elapsed());
    cx.update(|_, cx| {
        let v = viewer.read(cx);
        let count = (0..500).filter(|&i| chat_body_visible(v, i)).count();
        assert!(count < 60, "only nearby messages should register selectable text, rendered {count}");
        assert!(chat_body_visible(v, 499), "newest message is visible");
        assert!(!chat_body_visible(v, 0), "oldest message is offscreen");
    });
    let position = cx.debug_bounds("rail-list").unwrap().center();
    cx.simulate_mouse_move(position, None, Default::default());
    cx.update(|window, cx| {
        window.dispatch_event(
            ScrollWheelEvent { position, delta: ScrollDelta::Pixels(point(px(0.), px(50_000.))), ..Default::default() }
                .to_platform_input(),
            cx,
        );
    });
    settle(cx);
    cx.update(|_, cx| {
        assert!(chat_body_visible(viewer.read(cx), 0), "old multiline messages remain readable by scrolling");
        assert!(!chat_body_visible(viewer.read(cx), 499), "the tail is no longer rendered at the beginning");
    });
    cx.update(|_, cx| {
        viewer.update(cx, |v, cx| {
            assert!(v.send_chat("message 500".into()));
            cx.notify();
        })
    });
    settle(cx);
    cx.update(|_, cx| {
        assert!(chat_body_visible(viewer.read(cx), 500), "sending a message follows the new tail");
        assert!(!chat_body_visible(viewer.read(cx), 0));
    });
    cx.update(|_, cx| {
        viewer.update(cx, |v, cx| {
            let key = ChatKey::User("alice".into());
            let Screen::Connected(conn) = &mut v.session.screen else { unreachable!() };
            conn.chat.add(
                key.clone(),
                ChatLine { from: "alice".into(), lines: vec!["private message".into()] },
                UNIX_EPOCH,
            );
            v.open_chat(key, cx);
        })
    });
    settle(cx);
    cx.update(|_, cx| {
        assert!(chat_body_visible(viewer.read(cx), 0), "switching conversations resets the list");
        assert!(!chat_body_visible(viewer.read(cx), 500));
    });
}

fn double_click_match(index: usize, cx: &mut VisualTestContext) {
    let selector =
        ["match-row-0", "match-row-1", "match-row-2", "match-row-3", "match-row-4", "match-row-5", "match-row-6"]
            [index];
    let bounds = cx.debug_bounds(selector).expect("the match row is visible");
    cx.simulate_event(MouseDownEvent {
        position: bounds.center(),
        button: MouseButton::Left,
        modifiers: Default::default(),
        click_count: 2,
        first_mouse: false,
    });
    cx.simulate_event(gpui_kit::MouseUpEvent {
        position: bounds.center(),
        button: MouseButton::Left,
        modifiers: Default::default(),
        click_count: 2,
    });
    settle(cx);
}

fn child_context(viewer: &Entity<Viewer>, id: &str, cx: &mut VisualTestContext) -> VisualTestContext {
    let handle = cx.update(|_, cx| viewer.read(cx).match_windows[id].0);
    let mut child = VisualTestContext::from_window(handle, &cx.cx);
    settle(&mut child);
    child
}

#[gpui_kit::test]
fn child_focus_browses_each_half_independently_and_survives_reopen(cx: &mut TestAppContext) {
    let (viewer, cx) = fixture(cx);
    add_synchro_pair(&viewer, cx);
    cx.update(|_, cx| {
        viewer.update(cx, |v, cx| {
            let Some((_, lobby)) = v.session.connected_mut() else { unreachable!() };
            lobby.apply(
                OsMsg::Update { id: ".20.0".into(), token: crate::othello::MoveToken::parse("e6//3").unwrap() },
                Instant::now(),
            );
            v.open_match_window(".20", false, cx);
            cx.notify();
        })
    });
    settle(cx);
    let mut child = child_context(&viewer, ".20", cx);
    let tab = child.debug_bounds("child-focus-tab").unwrap();
    child.simulate_click(tab.center(), Default::default());
    settle(&mut child);
    assert!(child.debug_bounds("focus-board").is_some());
    assert!(child.debug_bounds("player-elapsed-.20.0").is_some());
    child.update(|window, cx| {
        window.press("home", cx);
        window.press("f", cx);
    });
    settle(&mut child);
    assert!(child.debug_bounds("focus-position-0").is_some());
    assert!(child.debug_bounds("player-elapsed-.20.0").is_none());
    assert!(
        child.debug_bounds("player-clock-.20.0-white").unwrap().left()
            < child.debug_bounds("player-clock-.20.0-black").unwrap().left()
    );
    assert!(
        cx.debug_bounds("player-elapsed-.20.0").is_some(),
        "child navigation leaves the main window at the latest position"
    );
    let half = child.debug_bounds("child-half-1").unwrap();
    child.simulate_click(half.center(), Default::default());
    settle(&mut child);
    assert!(child.debug_bounds("focus-position-2").is_some(), "the second half starts at its own latest position");
    child.update(|window, cx| window.press("left", cx));
    settle(&mut child);
    assert!(child.debug_bounds("focus-position-1").is_some());
    let half = child.debug_bounds("child-half-0").unwrap();
    child.simulate_click(half.center(), Default::default());
    settle(&mut child);
    assert!(child.debug_bounds("focus-position-0").is_some(), "the first half retains its own history cursor");
    let tab = child.debug_bounds("child-synchro-tab").unwrap();
    child.simulate_click(tab.center(), Default::default());
    settle(&mut child);
    child.update(|window, cx| {
        window.press("home", cx);
        window.press("f", cx);
    });
    settle(&mut child);
    assert!(child.debug_bounds("player-elapsed-.20.0").is_some());
    assert!(
        child.debug_bounds("player-clock-.20.0-black").unwrap().left()
            < child.debug_bounds("player-clock-.20.0-white").unwrap().left(),
        "Synchro remains canonical and live"
    );
    let tab = child.debug_bounds("child-focus-tab").unwrap();
    child.simulate_click(tab.center(), Default::default());
    settle(&mut child);
    cx.update(|_, cx| {
        viewer.update(cx, |v, cx| {
            let Some((_, lobby)) = v.session.connected_mut() else { unreachable!() };
            lobby.apply(
                OsMsg::Update { id: ".20.0".into(), token: crate::othello::MoveToken::parse("f3//4").unwrap() },
                Instant::now(),
            );
            cx.notify();
        })
    });
    settle(&mut child);
    assert!(child.debug_bounds("focus-position-0").is_some(), "live updates preserve a child browsing history");
    cx.update(|_, cx| viewer.update(cx, |v, cx| v.open_match_window(".20", false, cx)));
    settle(cx);
    settle(&mut child);
    assert!(child.debug_bounds("focus-position-0").is_some(), "reopening the same match preserves its Focus state");
    child.update(|window, cx| window.press("end", cx));
    settle(&mut child);
    assert!(child.debug_bounds("focus-position-4").is_some());
}

#[gpui_kit::test]
fn window_icons_preserve_main_selection_and_open_async_archives(cx: &mut TestAppContext) {
    let (viewer, cx) = fixture(cx);
    cx.update(|_, cx| {
        viewer.update(cx, |v, cx| {
            let Some((_, lobby)) = v.session.connected_mut() else { unreachable!() };
            lobby.matches = vec![crate::proto::MatchEntry::parse(".20 1800 alice 1900 bob s8r10 U")];
            lobby.view = ViewMode::Focus;
            v.rail = Rail::Matches;
            cx.notify();
        })
    });
    settle(cx);
    let button = cx.debug_bounds("match-window-0").unwrap();
    cx.simulate_click(button.center(), Default::default());
    settle(cx);
    let mut live = child_context(&viewer, ".20", cx);
    assert!(live.debug_bounds("synchro-waiting-0").is_some());
    cx.update(|_, cx| {
        viewer.update(cx, |v, cx| {
            let Some((_, lobby)) = v.session.connected_mut() else { unreachable!() };
            assert_eq!(lobby.focus.as_deref(), Some(".12"));
            assert_eq!(lobby.view, ViewMode::Focus);
            let archived = crate::proto::Archived {
                id: ".20".into(),
                date: "today".into(),
                player1: "alice".into(),
                player2: "bob".into(),
                score: Some(2.),
                info: "s8".into(),
            };
            lobby.history.insert("alice".into(), vec![archived.clone()]);
            lobby.stored.insert("alice".into(), vec![crate::proto::Archived { id: ".21".into(), ..archived }]);
            v.sheet = Some(super::Sheet::Games("alice".into()));
            cx.notify();
        })
    });
    settle(cx);
    let button = cx.debug_bounds("archive-window-FINISHED-.20").unwrap();
    cx.simulate_click(button.center(), Default::default());
    settle(cx);
    let mut archive = child_context(&viewer, "archive .20", cx);
    assert!(archive.debug_bounds("match-archive-loading").is_some());
    cx.simulate_click(button.center(), Default::default());
    settle(cx);
    assert_eq!(cx.cx.windows().len(), 3, "pending archive reuses its window and remains separate from live ID");
    cx.update(|_, cx| {
        viewer.update(cx, |v, cx| {
            let Some((_, lobby)) = v.session.connected_mut() else { unreachable!() };
            assert_eq!(lobby.focus.as_deref(), Some(".12"));
            assert_eq!(lobby.view, ViewMode::Focus);
            assert!(matches!(v.sheet, Some(super::Sheet::Games(_))));
            let record = GameRecord::parse(SPEC_JOIN).unwrap();
            lobby.apply(OsMsg::Look(vec![record.clone(), record]), Instant::now());
            assert_eq!(lobby.focus.as_deref(), Some(".12"));
            cx.notify();
        })
    });
    settle(cx);
    settle(&mut archive);
    assert!(archive.debug_bounds("synchro-board-archive .20.0").is_some());
    assert!(archive.debug_bounds("synchro-board-archive .20.1").is_some());
    let tab = archive.debug_bounds("child-focus-tab").unwrap();
    archive.simulate_click(tab.center(), Default::default());
    settle(&mut archive);
    archive.update(|window, cx| window.press("home", cx));
    settle(&mut archive);
    assert!(archive.debug_bounds("focus-position-0").is_some(), "an archived game supports Focus history navigation");
    archive.update(|window, cx| window.press("right", cx));
    settle(&mut archive);
    assert!(archive.debug_bounds("focus-position-1").is_some());
    let button = cx.debug_bounds("archive-window-ADJOURNED-.21").unwrap();
    cx.simulate_click(button.center(), Default::default());
    settle(cx);
    let mut missing = child_context(&viewer, "archive .21", cx);
    assert!(missing.debug_bounds("match-archive-loading").is_some());
    cx.update(|_, cx| {
        viewer.update(cx, |v, cx| {
            let Some((_, lobby)) = v.session.connected_mut() else { unreachable!() };
            lobby.apply(OsMsg::LookMissing { id: ".21".into() }, Instant::now());
            cx.notify();
        })
    });
    settle(&mut missing);
    assert!(missing.debug_bounds("match-archive-unavailable").is_some());
    archive.update(|window, _| window.remove_window());
    settle(cx);
    assert!(cx.debug_bounds("archive-window-FINISHED-.20").is_some(), "closing a child leaves the history page open");
}

#[gpui_kit::test]
fn back_returns_to_the_games_page_a_game_was_opened_from(cx: &mut TestAppContext) {
    let (viewer, cx) = fixture(cx);
    assert!(cx.debug_bounds("back").is_none(), "nothing to go back to yet");
    cx.update(|_, cx| {
        viewer.update(cx, |v, cx| {
            let Some((_, lobby)) = v.session.connected_mut() else { unreachable!() };
            let archived = crate::proto::Archived {
                id: ".20".into(),
                date: "today".into(),
                player1: "alice".into(),
                player2: "bob".into(),
                score: Some(2.),
                info: "s8".into(),
            };
            lobby.history.insert("alice".into(), vec![archived]);
            v.sheet = Some(super::Sheet::Games("alice".into()));
            cx.notify();
        })
    });
    settle(cx);
    let row = cx.debug_bounds("archive-window-FINISHED-.20").unwrap();
    cx.simulate_click(point(row.left() - px(200.), row.center().y), Default::default());
    settle(cx);
    assert!(cx.debug_bounds("archive-window-FINISHED-.20").is_none(), "the row opens the game in place of the page");
    let back = cx.debug_bounds("back").expect("a game opened from the page offers Back");
    cx.simulate_click(back.center(), Default::default());
    settle(cx);
    assert!(cx.debug_bounds("archive-window-FINISHED-.20").is_some());
    assert!(cx.debug_bounds("back").is_none());
}

#[gpui_kit::test]
fn match_windows_open_once_and_share_live_boards_without_main_view_state(cx: &mut TestAppContext) {
    let (viewer, cx) = fixture(cx);
    add_synchro_pair(&viewer, cx);
    cx.update(|_, cx| {
        viewer.update(cx, |v, cx| {
            let Some((_, lobby)) = v.session.connected_mut() else { unreachable!() };
            lobby.matches = vec![
                crate::proto::MatchEntry::parse(".12 1800 alice 1900 bob 8 U"),
                crate::proto::MatchEntry::parse(".20 1800 alice 1900 bob s8r10 U"),
            ];
            v.rail = Rail::Matches;
            cx.notify();
        })
    });
    settle(cx);
    double_click_match(0, cx);
    assert_eq!(cx.cx.windows().len(), 2);
    cx.update(|_, cx| {
        viewer.update(cx, |v, cx| {
            v.open_match_window(".13", false, cx);
            v.open_match_window(".13", false, cx);
        })
    });
    settle(cx);
    assert_eq!(cx.cx.windows().len(), 3, "two queued opens create one window");
    let mut queued = child_context(&viewer, ".13", cx);
    queued.update(|window, _| window.remove_window());
    settle(cx);
    double_click_match(0, cx);
    assert_eq!(cx.cx.windows().len(), 2, "opening the same match activates its existing window");
    let mut ordinary = child_context(&viewer, ".12", cx);
    assert!(ordinary.debug_bounds("focus-board").is_some());
    assert!(ordinary.debug_bounds("synchro-waiting-1").is_none());
    ordinary.simulate_resize(size(px(520.), px(720.)));
    settle(&mut ordinary);
    let board = ordinary.debug_bounds("focus-board").unwrap();
    assert!(board.left() >= px(0.) && board.right() <= px(520.));
    assert!(board.bottom() <= px(720.), "single board fits in a resized window: {board:?}");
    let card = ordinary.debug_bounds("child-focus-card").unwrap();
    assert!(card.bottom() <= px(720.), "the footer also fits: {card:?}");
    double_click_match(1, cx);
    let mut paired = child_context(&viewer, ".20", cx);
    assert_eq!(cx.cx.windows().len(), 3);
    for selector in ["synchro-board-.20.0", "synchro-board-.20.1"] {
        assert!(paired.debug_bounds(selector).is_some());
    }
    cx.update(|_, cx| {
        viewer.update(cx, |v, cx| {
            let Some((_, lobby)) = v.session.connected_mut() else { unreachable!() };
            lobby.apply(
                OsMsg::Update { id: ".20.0".into(), token: crate::othello::MoveToken::parse("e6//3").unwrap() },
                Instant::now(),
            );
            v.board.views.navigate(lobby.focused().unwrap(), crate::game::Nav::First);
            v.board.views.get_mut(lobby.focused().unwrap()).flipped = true;
            cx.notify();
        })
    });
    settle(cx);
    settle(&mut paired);
    assert!(cx.debug_bounds("player-elapsed-.20.0").is_none(), "main Focus is browsing the start");
    assert!(
        paired.debug_bounds("player-elapsed-.20.0").is_some(),
        "child sees the canonical update at the live position"
    );
    paired.simulate_resize(size(px(1120.), px(720.)));
    settle(&mut paired);
    for selector in ["synchro-board-.20.0", "synchro-board-.20.1"] {
        let board = paired.debug_bounds(selector).unwrap();
        assert!(
            board.left() >= px(0.) && board.right() <= px(1120.) && board.bottom() <= px(720.),
            "{selector} fits: {board:?}"
        );
    }
    ordinary.update(|window, _| window.remove_window());
    settle(cx);
    settle(&mut paired);
    assert_eq!(cx.cx.windows().len(), 2, "closing a child retains the main window and other child");
    cx.update(|_, cx| {
        let v = viewer.read(cx);
        assert!(!v.match_windows.contains_key(".12"));
        let Some((_, lobby)) = v.session.connected() else { unreachable!() };
        assert!(!lobby.is_watched(".12"), "closing the last view stops its subscription");
        assert!(lobby.is_watched(".20"), "the main window and other child keep their subscription");
    });
}

#[gpui_kit::test]
fn finished_window_buttons_open_held_games_or_request_the_archive(cx: &mut TestAppContext) {
    let (viewer, cx) = fixture(cx);
    cx.update(|_, cx| {
        viewer.update(cx, |v, cx| {
            let Some((_, lobby)) = v.session.connected_mut() else { unreachable!() };
            for (id, archive) in [(".12", None), (".30", Some(".90"))] {
                let crate::proto::MatchEntry::Summary(summary) =
                    crate::proto::MatchEntry::parse(&format!("{id} 1800 alice 1900 bob 8 U"))
                else {
                    panic!()
                };
                lobby.apply(
                    OsMsg::MatchRemove {
                        id: id.into(),
                        finished: Some(crate::proto::Finished {
                            summary,
                            outcome: crate::proto::Outcome::Aborted,
                            archive: archive.map(str::to_string),
                        }),
                    },
                    Instant::now(),
                );
            }
            lobby.games.get_mut(".12").unwrap().phase = crate::game::Phase::Over(crate::proto::Outcome::Aborted);
            lobby.watched.clear();
            lobby.view = ViewMode::Focus;
            v.rail = Rail::Matches;
            cx.notify();
        })
    });
    settle(cx);
    for selector in ["finished-window-0", "finished-window-1"] {
        let button = cx.debug_bounds(selector).unwrap();
        cx.simulate_click(button.center(), Default::default());
        settle(cx);
    }
    let mut held = child_context(&viewer, ".12", cx);
    assert!(held.debug_bounds("focus-board").is_some());
    let mut archive = child_context(&viewer, "archive .90", cx);
    assert!(archive.debug_bounds("match-archive-loading").is_some());
    cx.update(|_, cx| {
        let Some((_, lobby)) = viewer.read(cx).session.connected() else { unreachable!() };
        assert!(lobby.watched.is_empty(), "ended games never subscribe as live matches");
        assert_eq!(lobby.view, ViewMode::Focus);
        assert_eq!(lobby.focus.as_deref(), Some(".12"));
    });
}

#[gpui_kit::test]
fn live_match_window_waits_for_a_recycled_number_before_binding_the_new_join(cx: &mut TestAppContext) {
    for existing_window in [false, true] {
        let (viewer, cx) = fixture(cx);
        if existing_window {
            cx.update(|_, cx| viewer.update(cx, |v, cx| v.open_match_window(".12", false, cx)));
            settle(cx);
        }
        cx.update(|_, cx| {
            viewer.update(cx, |v, cx| {
                v.session.lobby.apply(OsMsg::MatchRemove { id: ".12".into(), finished: None }, Instant::now());
                v.session.lobby.apply(
                    OsMsg::MatchAdd(crate::proto::MatchEntry::parse(".12 1800 carol 1900 dave 8 U")),
                    Instant::now(),
                );
                v.watch_in_window(".12", cx);
            })
        });
        settle(cx);
        let mut child = child_context(&viewer, ".12", cx);
        assert!(
            child.debug_bounds("focus-board").is_none(),
            "the completed predecessor must not be bound while waiting for Join"
        );
        assert!(child.debug_bounds("match-live-loading").is_some());
        cx.update(|_, cx| {
            viewer.update(cx, |v, cx| {
                let mut record = GameRecord::parse(SPEC_JOIN).unwrap();
                record.black.name = "carol".into();
                record.white.name = "dave".into();
                v.session.lobby.apply(OsMsg::Join { id: ".12".into(), record: Box::new(record) }, Instant::now());
                cx.notify();
            })
        });
        settle(&mut child);
        assert!(child.debug_bounds("focus-board").is_some(), "one explicit open binds the new match after Join");
        assert!(child.debug_bounds("match-replaced").is_none());
    }
}

#[gpui_kit::test]
fn live_synchro_window_excludes_each_completed_predecessor_half(cx: &mut TestAppContext) {
    for first_join_before_open in [false, true] {
        let (viewer, cx) = fixture(cx);
        cx.update(|_, cx| {
            viewer.update(cx, |v, cx| {
                for half in [0, 1] {
                    let mut record = GameRecord::parse(SPEC_JOIN).unwrap();
                    record.game_type = "s8r10".into();
                    v.session
                        .lobby
                        .apply(OsMsg::Join { id: format!(".20.{half}"), record: Box::new(record) }, Instant::now());
                }
                v.session.lobby.apply(OsMsg::MatchRemove { id: ".20".into(), finished: None }, Instant::now());
                v.session.lobby.apply(
                    OsMsg::MatchAdd(crate::proto::MatchEntry::parse(".20 1800 carol 1900 dave s8r10 U")),
                    Instant::now(),
                );
                if first_join_before_open {
                    let mut record = GameRecord::parse(SPEC_JOIN).unwrap();
                    record.game_type = "s8r10".into();
                    record.date = Some("next match".into());
                    v.session.lobby.apply(OsMsg::Join { id: ".20.0".into(), record: Box::new(record) }, Instant::now());
                }
                v.watch_in_window(".20", cx);
            })
        });
        settle(cx);
        let mut child = child_context(&viewer, ".20", cx);
        assert_eq!(child.debug_bounds("synchro-board-.20.0").is_some(), first_join_before_open);
        assert!(child.debug_bounds("synchro-board-.20.1").is_none());
        assert!(child.debug_bounds("synchro-waiting-1").is_some());
        cx.update(|_, cx| {
            viewer.update(cx, |v, cx| {
                for half in [0, 1] {
                    let mut record = GameRecord::parse(SPEC_JOIN).unwrap();
                    record.game_type = "s8r10".into();
                    record.date = Some("next match".into());
                    v.session
                        .lobby
                        .apply(OsMsg::Join { id: format!(".20.{half}"), record: Box::new(record) }, Instant::now());
                }
                cx.notify();
            })
        });
        settle(&mut child);
        assert!(child.debug_bounds("synchro-board-.20.0").is_some());
        assert!(child.debug_bounds("synchro-board-.20.1").is_some());
        assert!(child.debug_bounds("match-replaced").is_none());
    }
}

#[gpui_kit::test]
fn match_window_waits_for_halves_and_rejects_a_recycled_match_until_reopened(cx: &mut TestAppContext) {
    let (viewer, cx) = fixture(cx);
    cx.update(|_, cx| {
        viewer.update(cx, |v, cx| {
            let Some((_, lobby)) = v.session.connected_mut() else { unreachable!() };
            lobby.matches = vec![crate::proto::MatchEntry::parse(".20 1800 alice 1900 bob s8r10 U")];
            v.rail = Rail::Matches;
            v.show_log = false;
            cx.notify();
        })
    });
    settle(cx);
    double_click_match(0, cx);
    let mut child = child_context(&viewer, ".20", cx);
    assert!(child.debug_bounds("synchro-waiting-0").is_some());
    assert!(child.debug_bounds("synchro-waiting-1").is_some());
    cx.update(|_, cx| {
        viewer.update(cx, |v, cx| {
            let Some((_, lobby)) = v.session.connected_mut() else { unreachable!() };
            let mut record = GameRecord::parse(SPEC_JOIN).unwrap();
            record.game_type = "s8r10".into();
            lobby.apply(OsMsg::Join { id: ".20.0".into(), record: Box::new(record) }, Instant::now());
            cx.notify();
        })
    });
    settle(&mut child);
    assert!(child.debug_bounds("synchro-board-.20.0").is_some());
    assert!(child.debug_bounds("synchro-waiting-1").is_some());
    cx.update(|_, cx| {
        viewer.update(cx, |v, cx| {
            let Some((_, lobby)) = v.session.connected_mut() else { unreachable!() };
            let mut record = GameRecord::parse(SPEC_JOIN).unwrap();
            record.game_type = "s8r10".into();
            record.date = Some("new match".into());
            lobby.apply(
                OsMsg::MatchAdd(crate::proto::MatchEntry::parse(".20 1800 alice 1900 bob s8r10 U")),
                Instant::now(),
            );
            lobby.apply(OsMsg::Join { id: ".20.0".into(), record: Box::new(record) }, Instant::now());
            cx.notify();
        })
    });
    settle(&mut child);
    assert!(child.debug_bounds("match-replaced").is_some());
    assert!(child.debug_bounds("synchro-board-.20.0").is_none());
    settle(cx);
    double_click_match(0, cx);
    settle(&mut child);
    assert_eq!(cx.cx.windows().len(), 2);
    assert!(child.debug_bounds("synchro-board-.20.0").is_some(), "an explicit reopen chooses the new match");
    cx.update(|_, cx| {
        viewer.update(cx, |v, cx| {
            let Some((_, lobby)) = v.session.connected_mut() else { unreachable!() };
            for command in lobby.unwatch(".20") {
                lobby.send_app(command, |_| true);
            }
            cx.notify();
        })
    });
    settle(&mut child);
    assert!(child.debug_bounds("match-stopped").is_some(), "stopping the shared subscription is explicit");
}

#[gpui_kit::test]
fn multiple_match_windows_follow_reconnects(cx: &mut TestAppContext) {
    let (viewer, cx) = fixture(cx);
    cx.update(|_, cx| {
        viewer.update(cx, |v, cx| {
            let Some((_, lobby)) = v.session.connected_mut() else { unreachable!() };
            lobby.matches.clear();
            for n in 30..37 {
                lobby.matches.push(crate::proto::MatchEntry::parse(&format!(".{n} 1800 alice 1900 bob 8 U")));
                lobby.apply(
                    OsMsg::Join { id: format!(".{n}"), record: Box::new(GameRecord::parse(SPEC_JOIN).unwrap()) },
                    Instant::now(),
                );
            }
            v.rail = Rail::Matches;
            v.show_log = false;
            cx.notify();
        })
    });
    settle(cx);
    for i in 0..7 {
        double_click_match(i, cx);
    }
    assert_eq!(cx.cx.windows().len(), 8, "seven child windows remain open simultaneously");
    let mut child = child_context(&viewer, ".30", cx);
    assert!(child.debug_bounds("focus-board").is_some());
    let saved = cx.update(|_, cx| {
        viewer.update(cx, |v, cx| {
            let retry = super::Retry {
                params: net::LoginParams {
                    host: "example.test".into(),
                    port: 5000,
                    login: "me".into(),
                    password: String::new(),
                    keepalive: net::KEEPALIVE,
                },
                attempt: 1,
                chat: Chat::new(None, "me".into()),
                _timer: Task::ready(()),
            };
            let saved = std::mem::replace(&mut v.session.screen, Screen::Reconnecting(retry));
            cx.notify();
            saved
        })
    });
    settle(&mut child);
    assert!(child.debug_bounds("match-reconnecting").is_some());
    assert!(child.debug_bounds("focus-board").is_none(), "a disconnected observer does not pretend clocks are live");
    cx.update(|_, cx| {
        viewer.update(cx, |v, cx| {
            v.session.screen = saved;
            cx.notify();
        })
    });
    settle(&mut child);
    assert!(child.debug_bounds("focus-board").is_some());
    cx.update(|_, cx| {
        viewer.update(cx, |v, cx| {
            v.session.screen = Screen::Login;
            cx.notify();
        })
    });
    settle(&mut child);
    assert!(child.debug_bounds("match-disconnected").is_some());
    assert_eq!(cx.cx.windows().len(), 8, "disconnect leaves child windows available for the next connection");
}

#[gpui_kit::test]
fn match_window_raw_half_and_undated_ended_board_stay_visible(cx: &mut TestAppContext) {
    let (viewer, cx) = fixture(cx);
    cx.update(|_, cx| {
        viewer.update(cx, |v, cx| {
            let Some((_, lobby)) = v.session.connected_mut() else { unreachable!() };
            lobby.matches = vec![crate::proto::MatchEntry::Raw(".20.0 awaiting details".into())];
            v.rail = Rail::Matches;
            v.show_log = false;
            cx.notify();
        })
    });
    settle(cx);
    double_click_match(0, cx);
    let mut child = child_context(&viewer, ".20", cx);
    assert!(child.debug_bounds("synchro-waiting-0").is_some());
    assert!(child.debug_bounds("synchro-waiting-1").is_some());
    cx.update(|_, cx| {
        viewer.update(cx, |v, cx| {
            let Some((_, lobby)) = v.session.connected_mut() else { unreachable!() };
            let mut record = GameRecord::parse(SPEC_JOIN).unwrap();
            record.date = None;
            record.game_type = "s8r10".into();
            lobby.apply(OsMsg::Join { id: ".20.0".into(), record: Box::new(record) }, Instant::now());
            lobby.apply(OsMsg::MatchRemove { id: ".20".into(), finished: None }, Instant::now());
            cx.notify();
        })
    });
    settle(&mut child);
    assert!(child.debug_bounds("synchro-board-.20.0").is_some());
    cx.update(|_, cx| viewer.update(cx, |_, cx| cx.notify()));
    settle(&mut child);
    assert!(
        child.debug_bounds("synchro-board-.20.0").is_some(),
        "an undated final board is the same match on subsequent renders"
    );
    assert!(child.debug_bounds("match-replaced").is_none());
    let tab = child.debug_bounds("child-focus-tab").unwrap();
    child.simulate_click(tab.center(), Default::default());
    settle(&mut child);
    let half = child.debug_bounds("child-half-1").unwrap();
    child.simulate_click(half.center(), Default::default());
    settle(&mut child);
    assert!(child.debug_bounds("focus-board").is_none(), "the missing half waits");
    let half = child.debug_bounds("child-half-0").unwrap();
    child.simulate_click(half.center(), Default::default());
    settle(&mut child);
    assert!(child.debug_bounds("focus-board").is_some(), "a missing half does not trap the board selector");
}

#[gpui_kit::test]
fn match_window_recycled_id_changes_board_count_only_after_explicit_reopen(cx: &mut TestAppContext) {
    let (viewer, cx) = fixture(cx);
    cx.update(|_, cx| {
        viewer.update(cx, |v, cx| {
            let Some((_, lobby)) = v.session.connected_mut() else { unreachable!() };
            lobby.matches = vec![crate::proto::MatchEntry::parse(".12 1800 alice 1900 bob 8 U")];
            v.rail = Rail::Matches;
            v.show_log = false;
            cx.notify();
        })
    });
    settle(cx);
    double_click_match(0, cx);
    let mut child = child_context(&viewer, ".12", cx);
    assert!(child.debug_bounds("focus-board").is_some());
    cx.update(|_, cx| {
        viewer.update(cx, |v, cx| {
            let Some((_, lobby)) = v.session.connected_mut() else { unreachable!() };
            lobby.matches = vec![crate::proto::MatchEntry::parse(".12 1800 carol 1900 dave s8 U")];
            for id in [".12.0", ".12.1"] {
                let mut record = GameRecord::parse(SPEC_JOIN).unwrap();
                record.game_type = "s8".into();
                lobby.apply(OsMsg::Join { id: id.into(), record: Box::new(record) }, Instant::now());
            }
            cx.notify();
        })
    });
    settle(cx);
    settle(&mut child);
    assert!(child.debug_bounds("match-replaced").is_some());
    assert!(child.debug_bounds("synchro-board-.12.0").is_none());
    double_click_match(0, cx);
    settle(&mut child);
    assert!(child.debug_bounds("synchro-board-.12.0").is_some());
    assert!(child.debug_bounds("synchro-board-.12.1").is_some());
    assert!(child.window_title().unwrap().contains("carol vs dave"));
    cx.update(|_, cx| {
        viewer.update(cx, |v, cx| {
            let Some((_, lobby)) = v.session.connected_mut() else { unreachable!() };
            lobby.matches = vec![crate::proto::MatchEntry::parse(".12 1800 eve 1900 frank 8 U")];
            cx.notify();
        })
    });
    settle(cx);
    settle(&mut child);
    assert!(child.debug_bounds("match-replaced").is_some());
    double_click_match(0, cx);
    cx.update(|_, cx| {
        viewer.update(cx, |v, cx| {
            let Some((_, lobby)) = v.session.connected_mut() else { unreachable!() };
            assert!(
                !lobby.games.contains_key(".12"),
                "the first .12 went with the synchro pair that recycled its number"
            );
            lobby.apply(
                OsMsg::Join { id: ".12".into(), record: Box::new(GameRecord::parse(SPEC_JOIN).unwrap()) },
                Instant::now(),
            );
            cx.notify();
        })
    });
    settle(cx);
    settle(&mut child);
    assert!(child.debug_bounds("focus-board").is_some());
    assert!(child.debug_bounds("synchro-board-.12.0").is_none());
    assert_eq!(cx.cx.windows().len(), 2);
}

#[gpui_kit::test]
fn the_focus_board_fills_the_center_height_without_overflowing(cx: &mut TestAppContext) {
    let (viewer, cx) = fixture(cx);
    // The synchro pair adds board chips to the head row, which changes the height left for the board.
    for synchro in [false, true] {
        if synchro {
            add_synchro_pair(&viewer, cx);
        }
        for log in [false, true] {
            cx.update(|_, cx| {
                viewer.update(cx, |v, cx| {
                    v.show_log = log;
                    cx.notify();
                })
            });
            for (w, h) in [(1240., 920.), (1240., 700.), (1600., 800.), (1000., 1000.), (1900., 1000.), (1240., 1200.)]
            {
                cx.simulate_resize(size(px(w), px(h)));
                settle(cx);
                let center = cx.debug_bounds("center").unwrap();
                let board = cx.debug_bounds("focus-board").unwrap();
                // The column's bottom padding.
                let left = f32::from(center.bottom() - cx.debug_bounds("board-controls").unwrap().bottom()) - 12.;
                let case = format!("synchro {synchro}, log {log}, {w}x{h}: {left}px left under the controls");
                assert!(left >= 0., "{case}");
                // Cells are whole pixels, so up to a pixel per row stays unused; otherwise the board is at its largest or as wide as the center.
                let full = f32::from(board.size.height) >= 8. * 60. + ui::COORD + ui::FRAME - 2.
                    || board.right() + px(40.) >= center.right();
                assert!(left < 10. || full, "{case}");
            }
        }
    }
}

#[gpui_kit::test]
fn side_by_side_synchro_cards_fill_the_center_height_without_overflowing(cx: &mut TestAppContext) {
    let (viewer, cx) = fixture(cx);
    add_synchro_pair(&viewer, cx);
    open_synchro(&viewer, cx);
    for (w, h) in [(1240., 920.), (1240., 700.), (1600., 800.), (1900., 1000.), (1900., 760.)] {
        cx.simulate_resize(size(px(w), px(h)));
        settle(cx);
        let center = cx.debug_bounds("center").unwrap();
        for (i, card, board) in
            [(0, "synchro-card-.20.0", "synchro-board-.20.0"), (1, "synchro-card-.20.1", "synchro-board-.20.1")]
        {
            let (card, board) = (cx.debug_bounds(card).unwrap(), cx.debug_bounds(board).unwrap());
            // The row's bottom padding.
            let left = f32::from(center.bottom() - card.bottom()) - 16.;
            let case = format!("card {i}, {w}x{h}: {left}px left under the card");
            assert!(left >= 0., "{case}");
            let full = f32::from(board.size.height) >= 8. * 60. + ui::COORD + ui::FRAME - 2.
                || board.size.width + px(40.) >= card.size.width;
            assert!(left < 10. || full, "{case}");
        }
    }
}

#[gpui_kit::test]
fn failed_connects_show_an_error_box_on_the_login_screen(cx: &mut TestAppContext) {
    let (viewer, cx) = fixture(cx);
    show_login(&viewer, "alice", cx);
    assert!(cx.debug_bounds("login-error").is_none());
    cx.update(|window, cx| {
        viewer.update(cx, |v, cx| {
            v.form.port.update(cx, |s, cx| s.set_value("50x0", window, cx));
            v.connect(cx);
        })
    });
    settle(cx);
    assert!(cx.debug_bounds("login-error").is_some(), "a bad port is an error");
    cx.update(|_, cx| {
        viewer.update(cx, |v, cx| {
            let (tx, rx) = async_channel::bounded(1);
            drop(rx);
            v.session.screen = Screen::Connecting(Conn {
                link: net::demo(tx),
                label: "test".into(),
                chat: Chat::new(None, "me".into()),
                params: None,
                attempt: 0,
                logged_in_at: None,
                _pump: Task::ready(()),
            });
            v.on_net(net::NetEvent::Closed { reason: "connection refused".into(), retry: false }, cx);
        })
    });
    settle(cx);
    cx.update(|_, cx| assert_eq!(viewer.read(cx).form.error.as_deref(), Some("disconnected: connection refused")));
    assert!(cx.debug_bounds("login-error").is_some(), "a refused connection is an error");
}

#[gpui_kit::test]
fn archive_view_must_survive_a_connection_restore(cx: &mut TestAppContext) {
    let (viewer, cx) = fixture(cx);
    cx.update(|_, cx| {
        viewer.update(cx, |v, cx| {
            v.look(".73".into(), cx);
            v.on_message(vec!["/os: look".into(), format!("1 {SPEC_JOIN}")], cx);
            let Screen::Connected(conn) = std::mem::replace(&mut v.session.screen, Screen::Login) else {
                unreachable!()
            };
            assert_eq!(v.session.lobby.focused().unwrap().id, "archive .73");
            v.session.lobby.suspend();
            v.session.screen = Screen::Connecting(conn);
            v.on_net(net::NetEvent::LoggedIn, cx);
            v.on_message(vec![": + /os 1".into()], cx);
            let Some((_, lobby)) = v.session.connected() else { unreachable!() };
            assert_eq!(lobby.focus.as_deref(), Some("archive .73"));
            assert!(
                lobby.focused().is_some(),
                "the archive focus was restored but its record was dropped; no look request is pending"
            );
        })
    });
}

#[gpui_kit::test]
fn regression_an_ack_spelled_in_a_chat_does_not_stop_watches(cx: &mut TestAppContext) {
    let (viewer, cx) = fixture(cx);
    cx.update(|_, cx| {
        viewer.update(cx, |v, cx| {
            // The assembler has already taken the `|` off the continuation line.
            v.on_message(vec!["bob: hi".into(), ": +ack:  tell /os watch % => tell /os watch %".into()], cx);
            let Some((_, lobby)) = v.session.connected() else { unreachable!() };
            assert!(lobby.is_watched(".12") && lobby.games.contains_key(".12"));
        })
    });
}

#[gpui_kit::test]
fn regression_requests_sent_while_os_is_away_do_not_take_later_replies(cx: &mut TestAppContext) {
    let (viewer, cx) = fixture(cx);
    cx.update(|_, cx| {
        viewer.update(cx, |v, cx| {
            v.on_message(vec![": - /os".into()], cx);
            v.look(".73".into(), cx);
            v.on_message(
                vec![": ERR user '/os' not found.".into(), ": +ack:  tell /os look .73 => tell /os look .73".into()],
                cx,
            );
            v.send(crate::lobby::AppCommand::Who(crate::proto::RatingType::Eight));
            v.on_message(
                vec![": ERR user '/os' not found.".into(), ": +ack:  tell /os who 8 a => tell /os who 8 a".into()],
                cx,
            );
            v.on_message(vec![": + /os 1".into()], cx);
            v.look(".74".into(), cx);
            v.on_message(vec!["/os: look".into(), format!("1 {SPEC_JOIN}")], cx);
            v.on_message(vec!["/os: look".into(), format!("1 {SPEC_JOIN}")], cx);
            v.on_message(vec!["/os: who 1".into(), "zed + 1500@100".into()], cx);
            let Some((_, lobby)) = v.session.connected() else { unreachable!() };
            assert!(
                lobby.games.contains_key("archive .73") && lobby.games.contains_key("archive .74"),
                "/os's arrival asks for .73 again, ahead of .74"
            );
            assert!(lobby.users().is_empty(), "the who that met the ERR is not waiting for a reply");
        })
    });
}

#[gpui_kit::test]
fn finished_window_must_not_open_a_recycled_live_id(cx: &mut TestAppContext) {
    let (viewer, cx) = fixture(cx);
    cx.update(|_, cx| {
        viewer.update(cx, |v, cx| {
            v.rail = Rail::Matches;
            v.on_message(vec!["/os: - match .12 1850 alice 1720 bob 8 R -4.00 .73".into()], cx);
            v.on_message(vec!["/os: + match .12 1500 carol 1500 dave 8 R".into()], cx);
            let current = SPEC_JOIN
                .replace("PB[alice]", "PB[carol]")
                .replace("PW[bob]", "PW[dave]")
                .replace("2006.08.17_12:00:00.GMT", "2006.08.17_12:01:00.GMT");
            v.on_message(vec![format!("/os: join .12 {current}")], cx);
            let Some((_, lobby)) = v.session.connected() else { unreachable!() };
            assert_eq!(lobby.finished[0].report.summary.player1, "alice");
            assert_eq!(lobby.games[".12"].black.name, "carol");
            cx.notify();
        })
    });
    settle(cx);
    let button = cx.debug_bounds("finished-window-0").unwrap();
    cx.simulate_click(button.center(), Default::default());
    settle(cx);
    cx.update(|_, cx| {
        let v = viewer.read(cx);
        assert!(v.log.iter().flat_map(|b| &b.lines).any(|line| line == "> tell /os look .73"),
            "the ended record was overwritten, so opening alice/bob must retrieve archive .73 instead of displaying carol/dave");
    });
}

#[gpui_kit::test]
fn synchro_child_hints_must_be_independent_of_the_main_window(cx: &mut TestAppContext) {
    let (viewer, cx) = fixture(cx);
    add_synchro_pair(&viewer, cx);
    cx.update(|_, cx| viewer.update(cx, |v, cx| v.open_match_window(".20", false, cx)));
    settle(cx);
    let mut child = child_context(&viewer, ".20", cx);
    settle(&mut child);
    assert!(child.debug_bounds("legal-hint-.20.0-4-5").is_some(), "e6 is initially shown as legal in the child");
    cx.update(|_, cx| {
        viewer.update(cx, |v, cx| {
            let Some((_, lobby)) = v.session.connected_mut() else { unreachable!() };
            v.board.views.get_mut(&lobby.games[".20.0"]).hints = false;
            cx.notify();
        })
    });
    settle(&mut child);
    assert!(
        child.debug_bounds("legal-hint-.20.0-4-5").is_some(),
        "the main window's Hints setting leaked into the child Synchro board"
    );
}

#[gpui_kit::test]
fn undo_clamps_both_windows_before_the_next_join(cx: &mut TestAppContext) {
    let (viewer, cx) = fixture(cx);
    cx.update(|_, cx| {
        viewer.update(cx, |v, cx| {
            v.navigate(crate::game::Nav::Prev, cx);
            v.open_match_window(".12", false, cx);
        })
    });
    settle(cx);
    let mut child = child_context(&viewer, ".12", cx);
    child.update(|window, cx| {
        window.press("left", cx);
        window.press("f", cx);
    });
    settle(&mut child);
    assert!(child.debug_bounds("focus-position-1").is_some());
    cx.update(|_, cx| {
        viewer.update(cx, |v, cx| {
            let undone = SPEC_JOIN.replace("W[c5/-1.23/4.5]", "").replace("12:00:00", "12:00:10");
            let restored = SPEC_JOIN.replace("12:00:00", "12:00:20");
            v.on_message(vec![format!("/os: join .12 {undone}")], cx);
            v.on_message(vec![format!("/os: join .12 {restored}")], cx);
            assert_eq!(v.board.views.get(&v.session.lobby.games[".12"]).cursor, None);
            cx.notify();
        })
    });
    settle(&mut child);
    assert!(
        child.debug_bounds("focus-position-2").is_some(),
        "an undo resumes live even if another join arrives before drawing"
    );
    assert!(child.debug_bounds("match-replaced").is_none());
    let white = child.debug_bounds("player-clock-.12-white").unwrap();
    let black = child.debug_bounds("player-clock-.12-black").unwrap();
    assert!(white.left() < black.left(), "a fresh send time preserves the child's flip");
}

fn sent(v: &Viewer, cmd: &str) -> usize {
    v.log.iter().flat_map(|b| &b.lines).filter(|line| *line == &format!("> {cmd}")).count()
}

fn click(selector: &'static str, cx: &mut VisualTestContext) {
    let bounds = cx.debug_bounds(selector).unwrap_or_else(|| panic!("{selector} is not rendered"));
    cx.simulate_click(bounds.center(), Default::default());
    settle(cx);
}

fn lines(l: &[&str]) -> Vec<String> {
    l.iter().map(|s| s.to_string()).collect()
}

/// Rows as they reach the app, with the `|` already removed (fixtures/live-tourney.txt, tournament 1 in round 2).
const TOURNEYS: &[&str] = &[
    "/td: tourneys: 2",
    "   id director  game #pl   round start             clock       type",
    "    1     root   /os   2   1/  2  now         1:00//0:30          8",
    "    2     root   /os   2   0/  2  1:48        1:00//0:30          8",
];

#[gpui_kit::test]
fn the_tournament_tab_lists_tournaments_and_a_row_opens_one(cx: &mut TestAppContext) {
    let (viewer, cx) = fixture(cx);
    assert!(
        ["rail-tab-0", "rail-tab-1", "rail-tab-2", "rail-tab-3", "rail-tab-4"]
            .iter()
            .all(|t| cx.debug_bounds(t).is_some())
    );
    cx.update(|_, cx| {
        viewer.update(cx, |v, cx| {
            v.rail = super::RAILS[3].0;
            assert!(v.rail == Rail::Tourneys);
            v.refresh_rail(false, cx);
            v.refresh_rail(false, cx);
            assert_eq!(sent(v, "tell /td tourneys"), 1);
            v.on_message(lines(TOURNEYS), cx);
        })
    });
    settle(cx);
    click("tourney-row-0", cx);
    cx.update(|_, cx| {
        let v = viewer.read(cx);
        assert!(matches!(&v.sheet, Some(super::Sheet::Tourney(id)) if id == "2"));
        assert_eq!(sent(v, "tell /td finger 2"), 1);
        assert_eq!(sent(v, "tell /td st 2") + sent(v, "tell /td rankings 2"), 0, "/td refuses both before the start");
    });
    cx.update(|_, cx| {
        viewer.update(cx, |v, cx| {
            v.on_message(
                lines(&[
                    "/td: finger: 2",
                    "t me finger: ",
                    "id        : 2",
                    "joining   : open",
                    "director  : root",
                    "rounds    : 0/2",
                    "style     : swiss",
                    "clock     : 1:00//0:30",
                    "type      : 8",
                    "players   : 3 nrb nra (bye) ",
                    "round 1 is being played",
                ]),
                cx,
            );
            assert_eq!(
                (sent(v, "tell /td st 2"), sent(v, "tell /td rankings 2")),
                (1, 1),
                "a finger showing the start asks for the rest"
            );
        })
    });
    click("tourney-row-1", cx);
    cx.update(|_, cx| {
        viewer.update(cx, |v, cx| {
            for cmd in ["tell /td finger 1", "tell /td st 1", "tell /td rankings 1"] {
                assert_eq!(sent(v, cmd), 1, "{cmd}");
            }
            v.on_message(
                lines(&[
                    "/td: st: schedule for tournament 1:",
                    "",
                    "round   1:",
                    "  1.1       nrb      nra    +0.0",
                    "round   2:",
                    "  2.1       nra      nrb       ?",
                ]),
                cx,
            );
            v.on_message(
                lines(&[
                    "/td: rankings: tournament 1",
                    " 0.5 ( 0  1  0) { 0.00 }      nra [0.5000]",
                    " 0.5 ( 0  1  0) { 0.00 }      nrb [0.5000]",
                ]),
                cx,
            );
        })
    });
    settle(cx);
    assert!(cx.debug_bounds("tourney-watch-2-1").is_none(), "no running /os match is known for the pairing yet");
    cx.update(|_, cx| viewer.update(cx, |v, cx| v.on_message(lines(&["/os: + match .46 2121 nra 2123 nrb 8b T"]), cx)));
    settle(cx);
    click("tourney-watch-2-1", cx);
    cx.update(|_, cx| {
        let v = viewer.read(cx);
        assert_eq!(sent(v, "tell /os watch + .46"), 1);
        assert!(
            v.sheet.is_none() && matches!(v.back.last(), Some(super::Sheet::Tourney(id)) if id == "1"),
            "Back returns to the tournament"
        );
    });
    let before = cx.update(|_, cx| {
        viewer.update(cx, |v, _| {
            v.session.reads.sent.clear();
            sent(v, "tell /td st 1")
        })
    });
    click("back", cx);
    cx.update(|_, cx| {
        assert_eq!(
            sent(viewer.read(cx), "tell /td st 1"),
            before + 1,
            "Back asks again for what changed while the board was shown"
        )
    });
    click("standing-1", cx);
    cx.update(|_, cx| assert!(matches!(&viewer.read(cx).sheet, Some(super::Sheet::Games(login)) if login == "nrb")));
}

#[gpui_kit::test]
fn tournament_events_refresh_what_is_on_screen_once_after_a_delay(cx: &mut TestAppContext) {
    let (viewer, cx) = fixture(cx);
    cx.update(|_, cx| {
        viewer.update(cx, |v, cx| {
            v.on_message(lines(&[".chat /td: Congratulations to nra, winner of tournament 1"]), cx);
            v.on_message(lines(&["/os: + match .46 2121 nra 2123 nrb 8b T"]), cx);
            assert!(v.session.reads.td_refresh.is_none(), "nothing about tournaments is on screen");
            assert!(
                v.log.iter().any(|b| b.lines == [".chat /td: Congratulations to nra, winner of tournament 1"]),
                "broadcasts stay in the log"
            );
            v.rail = Rail::Tourneys;
            v.on_message(lines(TOURNEYS), cx);
            v.open_tourney("1".into(), cx);
            v.on_message(lines(&["/os: + match .47 2121 nra 2123 nrb 8b U"]), cx);
            assert!(v.session.reads.td_refresh.is_none(), "an unrated match is not a tournament's");
            v.on_message(lines(&["/os: - match .46 2221 nra 2025 nrb 8b T +18.00  .376"]), cx);
            let due = v.session.reads.td_refresh.map(|p| p.due);
            assert!(due.is_some());
            v.on_message(
                lines(&[
                    ".tourney /td: ending round 2 of tournament 1",
                    "",
                    "round   2:",
                    "  2.1       nra      nrb   +18.0",
                ]),
                cx,
            );
            assert_eq!(v.session.reads.td_refresh.map(|p| p.due), due, "a burst is one refresh");
            v.session.reads.td_refresh.as_mut().unwrap().due = Instant::now();
        })
    });
    cx.cx.executor().advance_clock(std::time::Duration::from_secs(1));
    settle(cx);
    cx.update(|_, cx| {
        viewer.update(cx, |v, _| {
            for cmd in ["tell /td finger 1", "tell /td st 1", "tell /td rankings 1"] {
                assert_eq!(sent(v, cmd), 1, "the earlier request's cooldown applies to event refreshes: {cmd}");
            }
            assert!(v.session.reads.td_refresh.is_some(), "the event still requires a refresh after the cooldown");
            assert!(
                v.session.reads.td_refresh.unwrap().due > Instant::now(),
                "schedule the next attempt at the cooldown, not on every tick"
            );
            for key in ["td 1", "td-results 1"] {
                v.session.reads.sent.insert(key.into(), Instant::now() - std::time::Duration::from_secs(5));
            }
            v.session.reads.td_refresh.as_mut().unwrap().due = Instant::now();
        })
    });
    cx.cx.executor().advance_clock(std::time::Duration::from_secs(1));
    settle(cx);
    cx.update(|_, cx| {
        let v = viewer.read(cx);
        for cmd in ["tell /td finger 1", "tell /td st 1", "tell /td rankings 1"] {
            assert_eq!(sent(v, cmd), 2, "{cmd}");
        }
        assert_eq!(sent(v, "tell /td tourneys"), 1, "the list was already refreshed for this event");
        assert!(v.session.reads.td_refresh.is_none(), "finishing a deferred refresh must not start polling");
    });
}

/// Tournament 3: a, b and c in a round robin with a rest each round, two rounds decided and the third being played.
const THREE: &[&str] = &[
    "/td: st: schedule for tournament 3:",
    "",
    "round   1:",
    "  1.1         a        b   +10.0",
    "  1.2         c    (bye)     bye",
    "round   2:",
    "  2.1         c        a    +4.0",
    "  2.2         b    (bye)     bye",
    "round   3:",
    "  3.1         b        c       ?",
    "  3.2         a    (bye)     bye",
];

fn open_three(viewer: &Entity<Viewer>, cx: &mut VisualTestContext) {
    open_three_as("rrobin", viewer, cx);
}

fn open_three_as(style: &str, viewer: &Entity<Viewer>, cx: &mut VisualTestContext) {
    cx.update(|_, cx| {
        viewer.update(cx, |v, cx| {
            v.on_message(
                lines(&[
                    "/td: finger: 3",
                    "t me finger: ",
                    "id        : 3",
                    "joining   : closed",
                    "director  : root",
                    "rounds    : 2/3",
                    &format!("style     : {style}"),
                    "clock     : 1:00//0:30",
                    "type      : 8",
                    "players   : 4 a b c (bye) ",
                    "round 3 is being played",
                ]),
                cx,
            );
            v.open_tourney("3".into(), cx);
            v.on_message(lines(THREE), cx);
            v.on_message(
                lines(&[
                    "/td: rankings: tournament 3",
                    " 2.0 ( 2  0  0) {   7.00 }        c [0.5000]",
                    " 1.0 ( 1  0  1) {   3.00 }        a [0.5000]",
                    " 1.0 ( 1  0  1) { -10.00 }        b [0.5000]",
                ]),
                cx,
            );
        })
    });
    settle(cx);
}

#[gpui_kit::test]
fn a_swiss_table_heads_its_games_with_rounds(cx: &mut TestAppContext) {
    let (viewer, cx) = fixture(cx);
    open_three_as("swiss", &viewer, cx);
    // Ranked c, a, b: c rested in round 1, a beat b.
    assert!(["round-0-0", "round-1-0", "round-2-0", "round-0-1"].iter().all(|c| cx.debug_bounds(c).is_some()));
    assert!(cx.debug_bounds("cross-1-2").is_none(), "no place columns in a Swiss table");
    let (name, cell) = (cx.debug_bounds("standing-1").unwrap(), cx.debug_bounds("round-1-0").unwrap());
    assert_eq!(name.bottom(), cell.bottom());
}

#[gpui_kit::test]
fn standings_show_movement_and_medals(cx: &mut TestAppContext) {
    let (viewer, cx) = fixture(cx);
    open_three(&viewer, cx);
    assert!(cx.debug_bounds("standing-move-0").is_none(), "c stayed first");
    assert!(
        cx.debug_bounds("standing-move-1").is_some() && cx.debug_bounds("standing-move-2").is_some(),
        "a fell to second, b rose to second"
    );
    assert!(["standing-medal-0", "standing-medal-1", "standing-medal-2"].iter().all(|m| cx.debug_bounds(m).is_some()));
    cx.update(|_, cx| {
        viewer.update(cx, |v, cx| {
            v.on_message(
                lines(&["/td: st: schedule for tournament 3:", "", "round   1:", "  1.1         a        b       ?"]),
                cx,
            );
        })
    });
    settle(cx);
    assert!(
        cx.debug_bounds("standing-medal-0").is_none() && cx.debug_bounds("standing-move-1").is_none(),
        "nothing decided yet"
    );
}

#[gpui_kit::test]
fn a_round_caption_shows_the_standings_after_that_round(cx: &mut TestAppContext) {
    let (viewer, cx) = fixture(cx);
    open_three(&viewer, cx);
    let click = |cx: &mut VisualTestContext, selector: &'static str| {
        let bounds = cx.debug_bounds(selector).unwrap();
        cx.simulate_click(bounds.center(), Default::default());
        settle(cx);
    };
    // Ranked c, a, b now; c and a met in round 2. After round 1 a leads on its win over b, c's rest
    // has no tiebreak, and round 2 is left out.
    let current = |cx: &mut VisualTestContext| cx.debug_bounds("cross-0-1").is_some();
    assert!(current(cx));
    click(cx, "round-caption-1");
    assert!(cx.debug_bounds("cross-0-2").is_some() && cx.debug_bounds("cross-0-1").is_none());
    click(cx, "round-caption-1");
    assert!(current(cx), "picking it again goes back");
    let shown = |cx: &mut VisualTestContext| viewer.read_with(cx, |v, _| v.tourney_round.clone());
    click(cx, "standings-round");
    click(cx, "standings-round-item-1");
    assert_eq!(shown(cx), Some(("3".into(), 1)), "the menu beside the standings picks a round too");
    // Then Up and Down step through the rounds, round 3 being /td's own standings.
    for (key, round) in [("up", Some(1)), ("down", Some(2)), ("down", None), ("down", None), ("up", Some(2))] {
        cx.simulate_keystrokes(key);
        settle(cx);
        assert_eq!(shown(cx), round.map(|n| ("3".into(), n)), "after {key}");
    }
}

#[gpui_kit::test]
fn the_standings_stay_put_while_a_long_schedule_scrolls(cx: &mut TestAppContext) {
    let (viewer, cx) = fixture(cx);
    open_three(&viewer, cx);
    let mut schedule = vec!["/td: st: schedule for tournament 3:".to_string(), String::new()];
    for r in 1..=30 {
        schedule.push(format!("round {r:3}:"));
        schedule.push(format!("  {r}.1 a b +2.0"));
        schedule.push(format!("  {r}.2 c (bye) bye"));
    }
    cx.update(|_, cx| viewer.update(cx, |v, cx| v.on_message(schedule, cx)));
    settle(cx);
    let (rounds, center) = (cx.debug_bounds("rounds").unwrap(), cx.debug_bounds("center").unwrap());
    assert!(rounds.bottom() <= center.bottom(), "the rounds scroll in their own box: {rounds:?} in {center:?}");
    let standing = cx.debug_bounds("standing-0").unwrap();
    assert!(cx.debug_bounds("round-caption-30").unwrap().top() > center.bottom(), "the last round is below");
    let position = rounds.center();
    cx.simulate_mouse_move(position, None, Default::default());
    for _ in 0..40 {
        cx.update(|window, cx| {
            window.dispatch_event(
                ScrollWheelEvent {
                    position,
                    delta: ScrollDelta::Pixels(point(px(0.), px(-400.))),
                    ..Default::default()
                }
                .to_platform_input(),
                cx,
            );
        });
        settle(cx);
    }
    let last = cx.debug_bounds("round-caption-30").unwrap();
    assert!(last.bottom() <= center.bottom(), "scrolled to the last round: {last:?}");
    assert_eq!(cx.debug_bounds("standing-0").unwrap(), standing, "the standings did not move");
    // The round menu holds all 30 in a box that scrolls.
    let picker = cx.debug_bounds("standings-round").unwrap();
    cx.simulate_click(picker.center(), Default::default());
    settle(cx);
    let first = cx.debug_bounds("standings-round-item-1").unwrap();
    let position = first.center();
    cx.simulate_mouse_move(position, None, Default::default());
    cx.update(|window, cx| {
        window.dispatch_event(
            ScrollWheelEvent { position, delta: ScrollDelta::Pixels(point(px(0.), px(-100.))), ..Default::default() }
                .to_platform_input(),
            cx,
        );
    });
    settle(cx);
    assert!(cx.debug_bounds("standings-round-item-1").unwrap().top() < first.top(), "the round menu scrolls");
    cx.simulate_keystrokes("escape");
    settle(cx);
    let before_last = cx.debug_bounds("round-caption-29").unwrap();
    cx.simulate_click(before_last.center(), Default::default());
    settle(cx);
    assert_eq!(cx.debug_bounds("standing-0").unwrap(), standing, "and stay in view after a pick");
    assert!(viewer.read_with(cx, |v, _| v.tourney_round == Some(("3".into(), 29))));
}

#[gpui_kit::test]
fn a_big_field_scrolls_its_standings_and_keeps_room_for_the_rounds(cx: &mut TestAppContext) {
    let (viewer, cx) = fixture(cx);
    let players: Vec<String> = (0..30).map(|i| format!("p{i:02}")).collect();
    let mut schedule = vec!["/td: st: schedule for tournament 5:".to_string(), String::new()];
    for r in 1..=10 {
        schedule.push(format!("round {r:3}:"));
        for b in 0..15 {
            schedule.push(format!("  {r}.{} {} {} +2.0", b + 1, players[2 * b], players[2 * b + 1]));
        }
    }
    let field = format!("players   : 30 {}", players.join(" "));
    cx.update(|_, cx| {
        viewer.update(cx, |v, cx| {
            v.on_message(
                lines(&[
                    "/td: finger: 5",
                    "t me finger: ",
                    "id        : 5",
                    "joining   : closed",
                    "director  : root",
                    "rounds    : 10/10",
                    "style     : swiss",
                    "clock     : 1:00//0:30",
                    "type      : 8",
                    &field,
                    "tournament has finished",
                ]),
                cx,
            );
            v.open_tourney("5".into(), cx);
            v.on_message(schedule, cx);
        })
    });
    settle(cx);
    let (rounds, center) = (cx.debug_bounds("rounds").unwrap(), cx.debug_bounds("center").unwrap());
    assert!(rounds.bottom() <= center.bottom(), "the rounds keep their room: {rounds:?} in {center:?}");
    let first = cx.debug_bounds("standing-0").unwrap();
    let position = first.center();
    cx.simulate_mouse_move(position, None, Default::default());
    cx.update(|window, cx| {
        window.dispatch_event(
            ScrollWheelEvent { position, delta: ScrollDelta::Pixels(point(px(0.), px(-200.))), ..Default::default() }
                .to_platform_input(),
            cx,
        );
    });
    settle(cx);
    assert!(cx.debug_bounds("standing-0").unwrap().top() < first.top(), "the standings scroll");
    assert_eq!(cx.debug_bounds("rounds").unwrap(), rounds, "in their own box");
}

#[gpui_kit::test]
fn the_games_per_pair_hold_whichever_round_is_shown(cx: &mut TestAppContext) {
    let (viewer, cx) = fixture(cx);
    open_three(&viewer, cx);
    // A double round robin half played: every pair meets twice, only the first meetings decided.
    let schedule = [
        "/td: st: schedule for tournament 3:",
        "",
        "round   1:",
        "  1.1 a b +2.0",
        "  1.2 c (bye) bye",
        "round   2:",
        "  2.1 c a -4.0",
        "  2.2 b (bye) bye",
        "round   3:",
        "  3.1 b c +6.0",
        "  3.2 a (bye) bye",
        "round   4:",
        "  4.1 b a ?",
        "  4.2 c (bye) bye",
        "round   5:",
        "  5.1 a c ?",
        "  5.2 b (bye) bye",
        "round   6:",
        "  6.1 c b ?",
        "  6.2 a (bye) bye",
    ];
    cx.update(|_, cx| viewer.update(cx, |v, cx| v.on_message(lines(&schedule), cx)));
    settle(cx);
    assert!(cx.debug_bounds("cross-games").is_some());
    for round in [1, 2, 5] {
        cx.update(|_, cx| viewer.update(cx, |v, cx| v.show_round("3", Some(round), cx)));
        settle(cx);
        assert!(cx.debug_bounds("cross-games").is_some(), "after round {round}");
    }
}

#[gpui_kit::test]
fn a_finished_tournament_drops_the_movement_arrows(cx: &mut TestAppContext) {
    let (viewer, cx) = fixture(cx);
    open_three(&viewer, cx);
    assert!(cx.debug_bounds("standing-move-1").is_some());
    cx.update(|_, cx| {
        viewer.update(cx, |v, cx| {
            v.on_message(lines(&["/td: tourneys: 1", "  3 root /os 4 3/3 done 1:00//0:30 8 Winner: c"]), cx);
        })
    });
    settle(cx);
    assert!(
        ["standing-move-0", "standing-move-1", "standing-move-2"].iter().all(|m| cx.debug_bounds(m).is_none()),
        "the final standings show no last-round rises or falls"
    );
}

#[gpui_kit::test]
fn the_crosstable_renders_each_pairing_and_scrolls_inside_the_center(cx: &mut TestAppContext) {
    let (viewer, cx) = fixture(cx);
    open_three(&viewer, cx);
    let cells = ["cross-0-1", "cross-1-0", "cross-0-2", "cross-2-0", "cross-1-2", "cross-2-1"];
    assert!(cells.iter().all(|c| cx.debug_bounds(c).is_some()), "every pair met or meets");
    assert!(["cross-0-0", "cross-1-1", "cross-2-2"].iter().all(|c| cx.debug_bounds(c).is_none()));
    let players: Vec<String> = (0..12).map(|i| format!("player{i}")).collect();
    let mut schedule = vec!["/td: st: schedule for tournament 4:".to_string(), String::new(), "round   1:".to_string()];
    for (i, b) in players.iter().enumerate() {
        for w in players.iter().skip(i + 1) {
            schedule.push(format!("  1.{} {b} {w} +2.0", schedule.len()));
            schedule.push(format!("  1.{} {w} {b} -2.0", schedule.len()));
        }
    }
    cx.update(|_, cx| {
        viewer.update(cx, |v, cx| {
            v.on_message(
                lines(&[
                    "/td: finger: 4",
                    "t me finger: ",
                    "id        : 4",
                    "joining   : closed",
                    "director  : root",
                    "rounds    : 1/1",
                    "style     : rrobin",
                    "clock     : 1:00//0:30",
                    "type      : 8",
                    &format!("players   : 12 {}", players.join(" ")),
                    "tournament has finished",
                ]),
                cx,
            );
            v.open_tourney("4".into(), cx);
            v.on_message(schedule, cx);
        })
    });
    settle(cx);
    let (name, cell) = (cx.debug_bounds("standing-11").unwrap(), cx.debug_bounds("cross-11-0").unwrap());
    assert_eq!(name.bottom(), cell.bottom(), "a player's name and games share one row: {name:?}, {cell:?}");
    for width in [700., 800., 1240.] {
        cx.simulate_resize(size(px(width), px(920.)));
        settle(cx);
        let (table, center) = (cx.debug_bounds("crosstable").unwrap(), cx.debug_bounds("center").unwrap());
        assert!(table.right() <= center.right(), "crosstable {table:?} overflows center {center:?}");
        let bottom = cx.debug_bounds("cross-11-0").unwrap();
        assert!(
            bottom.bottom() + px(16.) <= table.bottom(),
            "the horizontal scrollbar sits below the last row: {bottom:?} in {table:?}"
        );
        let last = cx.debug_bounds("cross-0-11").unwrap();
        assert!(
            width > 1000. || last.right() > table.right(),
            "at 800 px the table is wider than its section and scrolls: {last:?}"
        );
        if width < 1000. {
            let position = table.center();
            cx.simulate_mouse_move(position, None, Default::default());
            cx.update(|window, cx| {
                window.dispatch_event(
                    ScrollWheelEvent {
                        position,
                        delta: ScrollDelta::Pixels(point(px(-5_000.), px(0.))),
                        ..Default::default()
                    }
                    .to_platform_input(),
                    cx,
                );
            });
            settle(cx);
            let scrolled = cx.debug_bounds("cross-0-11").unwrap();
            assert!(
                scrolled.right() <= table.right() + px(1.) && scrolled.right() < last.right(),
                "scrolling brings the last column into view: {scrolled:?} in {table:?}"
            );
            assert_eq!(cx.debug_bounds("crosstable").unwrap().origin.y, table.origin.y, "the page stays put");
            cx.update(|window, cx| {
                window.dispatch_event(
                    ScrollWheelEvent {
                        position,
                        delta: ScrollDelta::Pixels(point(px(5_000.), px(0.))),
                        ..Default::default()
                    }
                    .to_platform_input(),
                    cx,
                );
            });
            settle(cx);
        }
    }
}

#[gpui_kit::test]
fn a_long_meeting_folds_into_a_score_so_every_column_fits(cx: &mut TestAppContext) {
    let (viewer, cx) = fixture(cx);
    let mut schedule = vec!["/td: st: schedule for tournament 5:".to_string(), String::new()];
    for round in 1..=24 {
        schedule.push(format!("round {round:>3}:"));
        schedule.push(format!("  {round}.1 a b {}", if round % 2 == 0 { "+2.0" } else { "-1.0" }));
        schedule.push(format!("  {round}.2 c d +0.0"));
    }
    cx.update(|_, cx| {
        viewer.update(cx, |v, cx| {
            v.on_message(
                lines(&[
                    "/td: finger: 5",
                    "t me finger: ",
                    "id        : 5",
                    "joining   : closed",
                    "director  : root",
                    "rounds    : 24/24",
                    "style     : rrobin",
                    "clock     : 1:00//0:30",
                    "type      : 8",
                    "players   : 4 a b c d",
                    "tournament has finished",
                ]),
                cx,
            );
            v.open_tourney("5".into(), cx);
            v.on_message(schedule, cx);
        })
    });
    settle(cx);
    let (table, last) = (cx.debug_bounds("crosstable").unwrap(), cx.debug_bounds("cross-3-2").unwrap());
    assert!(last.right() <= table.right(), "24 games per pair still fit four columns: {last:?} in {table:?}");
    assert!(cx.debug_bounds("cross-0-1").unwrap().size.width < px(100.));
    assert!(cx.debug_bounds("cross-games").is_some(), "the shared 24 is said once above the table");
}

#[gpui_kit::test]
fn a_tournament_watches_its_live_games_as_tiles_without_moving_the_board(cx: &mut TestAppContext) {
    let (viewer, cx) = fixture(cx);
    let watches = |v: &Viewer| (sent(v, "tell /os watch + .46"), sent(v, "tell /os watch - .46"));
    cx.update(|_, cx| {
        viewer.update(cx, |v, cx| {
            v.session.lobby.focus = None;
            v.on_message(lines(TOURNEYS), cx);
            v.open_tourney("1".into(), cx);
            v.on_message(
                lines(&[
                    "/td: st: schedule for tournament 1:",
                    "",
                    "round   1:",
                    "  1.1       nrb      nra    +0.0",
                    "round   2:",
                    "  2.1       nra      nrb       ?",
                ]),
                cx,
            );
            assert_eq!(watches(v), (0, 0), "no /os match is known for the pairing yet");
            v.on_message(lines(&["/os: + match .46 2121 nra 2123 nrb 8b T"]), cx);
            assert_eq!(watches(v), (1, 0), "a new tournament match is watched for its tile");
        })
    });
    settle(cx);
    assert!(
        cx.debug_bounds("tourney-tile-.46").is_some() && cx.debug_bounds("tourney-tile-board-.46").is_none(),
        "waiting for the join"
    );
    cx.update(|_, cx| {
        viewer.update(cx, |v, cx| {
            v.on_message(vec![format!("/os: join .46 {SPEC_JOIN}")], cx);
            assert_eq!(v.session.lobby.focus, None, "a tile's join does not take the board");
            v.session.watch_cleanup_at = Some(Instant::now() - std::time::Duration::from_secs(5));
        })
    });
    cx.cx.executor().advance_clock(std::time::Duration::from_secs(1));
    settle(cx);
    assert!(cx.debug_bounds("tourney-tile-board-.46").is_some());
    assert!(cx.debug_bounds("tourney-tile-eval-.46-WHITE").is_some(), "white reported -1.23 with its move");
    assert!(cx.debug_bounds("tourney-tile-eval-.46-BLACK").is_none(), "black's move came without an evaluation");
    cx.update(|_, cx| {
        let v = viewer.read(cx);
        assert_eq!(watches(v), (1, 0), "the open sheet keeps its tiles past the join's grace");
        assert!(
            v.session.lobby.is_watched(".46") && !v.session.lobby.is_watched(".12"),
            "other hidden watches still go"
        );
    });
    cx.update(|_, cx| {
        viewer.update(cx, |v, cx| {
            v.sheet = None;
            cx.notify();
        })
    });
    cx.cx.executor().advance_clock(std::time::Duration::from_secs(1));
    settle(cx);
    cx.update(|_, cx| {
        viewer.update(cx, |v, _| {
            assert_eq!(watches(v), (1, 0), "closing starts the grace");
            assert!(v.session.watch_cleanup_at.is_some());
            v.session.watch_cleanup_at = Some(Instant::now() - std::time::Duration::from_secs(5));
        })
    });
    cx.cx.executor().advance_clock(std::time::Duration::from_secs(1));
    settle(cx);
    cx.update(|_, cx| {
        viewer.update(cx, |v, cx| {
            assert_eq!(watches(v), (1, 1), "the tile's watch is released");
            assert_eq!(v.session.lobby.focus, None);
            v.open_tourney("1".into(), cx);
            assert_eq!(watches(v), (2, 1), "reopening watches the tile again");
        })
    });
    settle(cx);
    click("tourney-tile-.46", cx);
    cx.update(|_, cx| {
        let v = viewer.read(cx);
        assert_eq!(v.session.lobby.focus.as_deref(), Some(".46"), "a click shows the tile's game on the board");
        assert!(v.sheet.is_none() && matches!(v.back.last(), Some(super::Sheet::Tourney(id)) if id == "1"));
    });
}

#[gpui_kit::test]
fn a_finished_round_keeps_its_tiles_until_the_next_round_starts(cx: &mut TestAppContext) {
    let (viewer, cx) = fixture(cx);
    let join = SPEC_JOIN.replace("PB[alice]", "PB[nra]").replace("PW[bob]", "PW[nrb]");
    cx.update(|_, cx| {
        viewer.update(cx, |v, cx| {
            v.on_message(lines(TOURNEYS), cx);
            v.open_tourney("1".into(), cx);
            v.on_message(
                lines(&["/td: st: schedule for tournament 1:", "", "round   2:", "  2.1       nra      nrb       ?"]),
                cx,
            );
            v.on_message(lines(&["/os: + match .46 2121 nra 2123 nrb 8b T"]), cx);
            v.on_message(vec![format!("/os: join .46 {join}")], cx);
            v.on_message(lines(&["/os: - match .46 2221 nra 2025 nrb 8b T +18.00  .376"]), cx);
        })
    });
    cx.cx.executor().advance_clock(std::time::Duration::from_secs(1));
    settle(cx);
    assert!(cx.debug_bounds("tourney-tile-board-.46").is_some(), "the finished board stays between rounds");
    cx.update(|_, cx| {
        let v = viewer.read(cx);
        assert!(!v.session.lobby.is_watched(".46"), "the server's watch ends with the match");
        assert_eq!(sent(v, "tell /os watch - .46"), 0);
    });
    cx.update(|_, cx| {
        viewer.update(cx, |v, cx| {
            v.on_message(
                lines(&["/td: st: schedule for tournament 1:", "", "round   3:", "  3.1       nrb      nra       ?"]),
                cx,
            );
            v.on_message(lines(&["/os: + match .47 2025 nrb 2221 nra 8b T"]), cx);
        })
    });
    cx.cx.executor().advance_clock(std::time::Duration::from_secs(1));
    settle(cx);
    assert!(cx.debug_bounds("tourney-tile-.47").is_some() && cx.debug_bounds("tourney-tile-.46").is_none());
    cx.update(|_, cx| {
        viewer.update(cx, |v, _| {
            assert!(!v.session.lobby.background.contains(".46"), "the next round's tiles replace it");
            assert_eq!(sent(v, "tell /os watch + .46"), 1, "the finished match is not watched again");
        })
    });
}

#[gpui_kit::test]
fn a_game_that_ends_before_the_rest_of_its_round_keeps_its_tile(cx: &mut TestAppContext) {
    let (viewer, cx) = fixture(cx);
    let join = |black: &str, white: &str| {
        SPEC_JOIN.replace("PB[alice]", &format!("PB[{black}]")).replace("PW[bob]", &format!("PW[{white}]"))
    };
    let schedule = |first: &str, second: &str| {
        lines(&[
            "/td: st: schedule for tournament 1:",
            "",
            "round   2:",
            &format!("  2.1       nra      nrb   {first}"),
            &format!("  2.2       nrc      nrd   {second}"),
        ])
    };
    cx.update(|_, cx| {
        viewer.update(cx, |v, cx| {
            v.on_message(lines(TOURNEYS), cx);
            v.open_tourney("1".into(), cx);
            v.on_message(schedule("?", "?"), cx);
            v.on_message(lines(&["/os: + match .46 2121 nra 2123 nrb 8b T"]), cx);
            v.on_message(lines(&["/os: + match .48 2121 nrc 2123 nrd 8b T"]), cx);
            v.on_message(vec![format!("/os: join .46 {}", join("nra", "nrb"))], cx);
            v.on_message(vec![format!("/os: join .48 {}", join("nrc", "nrd"))], cx);
        })
    });
    settle(cx);
    let both = |cx: &mut VisualTestContext| {
        ["tourney-tile-board-.46", "tourney-tile-board-.48"].map(|t| cx.debug_bounds(t).is_some())
    };
    assert_eq!(both(cx), [true, true]);
    for (step, (first, second, removed)) in
        [("+18.0", "?", ".46 2221 nra 2025 nrb"), ("+18.0", "-64.0", ".48 2221 nrc 2025 nrd")].into_iter().enumerate()
    {
        cx.update(|_, cx| {
            viewer.update(cx, |v, cx| {
                v.on_message(lines(&[&format!("/os: - match {removed} 8b T +18.00")]), cx);
                v.on_message(schedule(first, second), cx);
            })
        });
        cx.cx.executor().advance_clock(std::time::Duration::from_secs(10));
        settle(cx);
        assert_eq!(both(cx), [true, true], "after game {} of the round ends, both boards stay", step + 1);
    }
}

#[gpui_kit::test]
fn back_retraces_every_page_opened_from_another_page(cx: &mut TestAppContext) {
    let (viewer, cx) = fixture(cx);
    cx.update(|_, cx| {
        viewer.update(cx, |v, cx| {
            v.on_message(lines(TOURNEYS), cx);
            v.open_tourney("1".into(), cx);
            v.on_message(
                lines(&[
                    "/td: rankings: tournament 1",
                    " 0.5 ( 0  1  0) { 0.00 }      nra [0.5000]",
                    " 0.5 ( 0  1  0) { 0.00 }      nrb [0.5000]",
                ]),
                cx,
            );
        })
    });
    settle(cx);
    click("standing-1", cx);
    click("refresh-games", cx);
    cx.update(|_, cx| viewer.update(cx, |v, cx| v.open_games("me".into(), cx)));
    settle(cx);
    cx.update(|_, cx| assert!(matches!(&viewer.read(cx).sheet, Some(super::Sheet::Games(login)) if login == "me")));
    click("back", cx);
    cx.update(|_, cx| {
        assert!(
            matches!(&viewer.read(cx).sheet, Some(super::Sheet::Games(login)) if login == "nrb"),
            "a refresh is not a page of its own"
        )
    });
    click("back", cx);
    cx.update(|_, cx| assert!(matches!(&viewer.read(cx).sheet, Some(super::Sheet::Tourney(id)) if id == "1")));
    assert!(cx.debug_bounds("back").is_none(), "the first page has nothing to go back to");
}

#[gpui_kit::test]
fn a_decided_pairing_opens_nothing(cx: &mut TestAppContext) {
    let (viewer, cx) = fixture(cx);
    cx.update(|_, cx| {
        viewer.update(cx, |v, cx| {
            v.on_message(lines(TOURNEYS), cx);
            v.open_tourney("1".into(), cx);
            v.on_message(
                lines(&[
                    "/td: st: schedule for tournament 1:",
                    "",
                    "round   1:",
                    "  1.1       nrb      nra    +0.0",
                    "round   2:",
                    "  2.1       nra      nrb       ?",
                ]),
                cx,
            );
        })
    });
    settle(cx);
    click("pairing-1-1", cx);
    cx.update(|_, cx| {
        let v = viewer.read(cx);
        assert_eq!(sent(v, "tell /os history nrb"), 0, "the server keeps too few games to find old ones");
        assert!(matches!(&v.sheet, Some(super::Sheet::Tourney(id)) if id == "1"));
    });
}

#[gpui_kit::test]
fn a_synchro_tournament_tile_shows_both_boards(cx: &mut TestAppContext) {
    let (viewer, cx) = fixture(cx);
    cx.update(|_, cx| {
        viewer.update(cx, |v, cx| {
            v.session.lobby.focus = None;
            v.on_message(lines(TOURNEYS), cx);
            v.open_tourney("1".into(), cx);
            v.on_message(
                lines(&["/td: st: schedule for tournament 1:", "", "round   1:", "  1.1       nrb      nrc       ?"]),
                cx,
            );
            v.on_message(lines(&["/os: + match .62 2085 nrb 1990 nrc s8 T"]), cx);
            assert_eq!(sent(v, "tell /os watch + .62"), 1);
            for board in [".62.0", ".62.1"] {
                v.on_message(vec![format!("/os: join {board} {SPEC_JOIN}")], cx);
            }
        })
    });
    settle(cx);
    for board in ["tourney-tile-board-.62.0", "tourney-tile-board-.62.1"] {
        assert!(cx.debug_bounds(board).is_some(), "{board}");
    }
    let (a, b) =
        (cx.debug_bounds("tourney-tile-board-.62.0").unwrap(), cx.debug_bounds("tourney-tile-board-.62.1").unwrap());
    assert!(a.right() <= b.left(), "the boards sit side by side in one tile");
}

#[gpui_kit::test]
fn service_restoration_refreshes_visible_tournaments(cx: &mut TestAppContext) {
    let (viewer, cx) = fixture(cx);
    cx.update(|_, cx| {
        viewer.update(cx, |v, cx| {
            v.rail = Rail::Tourneys;
            v.on_message(lines(TOURNEYS), cx);
            v.open_tourney("1".into(), cx);
            v.on_message(
                lines(&["/td: st: schedule for tournament 1:", "", "round   1:", "  1.1       nrb      nra    +0.0"]),
                cx,
            );
            v.on_message(lines(&[": - /os 1"]), cx);
            v.on_message(lines(&[": + /os 1"]), cx);
            assert_eq!(sent(v, "tell /td finger 1"), 2, "the visible tournament reloads after a service loss");
            assert_eq!(sent(v, "tell /td tourneys"), 1);
            v.on_message(lines(&[": + /os 1"]), cx);
            assert_eq!(sent(v, "tell /td finger 1"), 2, "duplicate service announcements send nothing");
            v.on_message(
                lines(&[
                    "/td: finger: 1",
                    "t me finger: ",
                    "id        : 1",
                    "joining   : closed",
                    "director  : root",
                    "rounds    : 1/2",
                    "style     : swiss",
                    "clock     : 1:00//0:30",
                    "type      : 8",
                    "players   : 2 nrb nra",
                    "round 2 is being played",
                ]),
                cx,
            );
            assert_eq!(sent(v, "tell /td st 1"), 2, "the restored finger reloads the schedule once");
            v.refresh_tourneys();
            v.refresh_rail(true, cx);
            assert_eq!(sent(v, "tell /td tourneys"), 1, "automatic and manual refreshes share a cooldown");
        })
    });
}

#[path = "local_tests.rs"]
mod local_tests;

#[gpui_kit::test]
fn tournament_list_before_finger_still_loads_missing_results_once(cx: &mut TestAppContext) {
    let (viewer, cx) = fixture(cx);
    cx.update(|_, cx| {
        viewer.update(cx, |v, cx| {
            v.open_tourney("1".into(), cx);
            assert_eq!(sent(v, "tell /td st 1"), 0, "the start is not known yet");
            v.on_message(lines(TOURNEYS), cx);
            let info = lines(&[
                "/td: finger: 1",
                "t me finger: ",
                "id        : 1",
                "joining   : closed",
                "director  : root",
                "rounds    : 1/2",
                "style     : swiss",
                "clock     : 1:00//0:30",
                "type      : 8",
                "players   : 2 nrb nra",
                "round 2 is being played",
            ]);
            v.on_message(info.clone(), cx);
            v.on_message(info, cx);
            assert_eq!(sent(v, "tell /td st 1"), 1);
            assert_eq!(
                sent(v, "tell /td rankings 1"),
                1,
                "finger repeats while results are pending do not duplicate requests"
            );
        })
    });
}

#[gpui_kit::test]
fn an_event_after_a_manual_refresh_is_not_lost_in_an_earlier_burst(cx: &mut TestAppContext) {
    let (viewer, cx) = fixture(cx);
    cx.update(|_, cx| {
        viewer.update(cx, |v, cx| {
            v.rail = Rail::Tourneys;
            v.on_message(lines(TOURNEYS), cx);
            v.open_tourney("1".into(), cx);
            v.session.reads.sent.clear();
            v.tourney_changed();
            v.refresh_rail(true, cx);
            v.open_tourney("1".into(), cx);
            v.on_message(lines(&[".tourney /td: ending round 2 of tournament 1"]), cx);
            v.refresh_tourneys();
            assert!(
                v.session.reads.td_refresh.is_some(),
                "the later event still needs a refresh after the manual requests"
            );
            assert_eq!(sent(v, "tell /td tourneys"), 1);
            assert_eq!(sent(v, "tell /td finger 1"), 2);
            for key in ["tourneys", "td 1", "td-results 1"] {
                v.session.reads.sent.insert(key.into(), Instant::now() - std::time::Duration::from_secs(5));
            }
            v.refresh_tourneys();
            assert_eq!(sent(v, "tell /td tourneys"), 2);
            assert_eq!(sent(v, "tell /td finger 1"), 3);
            assert!(v.session.reads.td_refresh.is_none());
        })
    });
}

#[gpui_kit::test]
fn continuous_tournament_events_do_not_starve_refresh(cx: &mut TestAppContext) {
    let (viewer, cx) = fixture(cx);
    cx.update(|_, cx| {
        viewer.update(cx, |v, cx| {
            v.rail = Rail::Tourneys;
            v.tourney_changed();
            v.session.reads.td_refresh.as_mut().unwrap().due = Instant::now();
            for _ in 0..4 {
                v.on_message(lines(&[".tourney /td: ending round 2 of tournament 1"]), cx);
            }
        })
    });
    cx.cx.executor().advance_clock(std::time::Duration::from_secs(1));
    settle(cx);
    cx.update(|_, cx| {
        assert_eq!(sent(viewer.read(cx), "tell /td tourneys"), 1, "events must not postpone an already due refresh")
    });
}

#[gpui_kit::test]
fn results_requested_by_a_reply_do_not_delay_finger(cx: &mut TestAppContext) {
    let (viewer, cx) = fixture(cx);
    cx.update(|_, cx| {
        viewer.update(cx, |v, cx| {
            v.open_tourney("1".into(), cx);
            v.session.reads.sent.insert("td 1".into(), Instant::now() - std::time::Duration::from_secs(5));
            v.tourney_changed();
            v.on_message(
                lines(&[
                    "/td: finger: 1",
                    "t me finger: ",
                    "id        : 1",
                    "joining   : closed",
                    "director  : root",
                    "rounds    : 1/2",
                    "style     : swiss",
                    "clock     : 1:00//0:30",
                    "type      : 8",
                    "players   : 2 nrb nra",
                    "round 2 is being played",
                ]),
                cx,
            );
            assert_eq!(sent(v, "tell /td st 1"), 1);
            v.refresh_tourneys();
            assert_eq!(sent(v, "tell /td finger 1"), 2, "st/rankings do not count as a finger request");
            assert_eq!(sent(v, "tell /td st 1"), 1, "refreshing finger does not duplicate pending results");
        })
    });
}

#[gpui_kit::test]
fn rejected_send_does_not_create_pending_watch_or_log(cx: &mut TestAppContext) {
    let (viewer, cx) = fixture(cx);
    cx.update(|_, cx| {
        viewer.update(cx, |v, _| {
            let Screen::Connected(conn) = &mut v.session.screen else { panic!("the fixture is connected") };
            conn.link.close();
            v.send(crate::lobby::AppCommand::Watch(".99".into()));
            assert!(!v.session.lobby.is_watched(".99"), "a rejected send cannot commit a pending subscription");
            assert_eq!(sent(v, "tell /os watch + .99"), 0, "only accepted commands are recorded as sent");
            assert!(!v.send_chat("unsent text".into()), "a rejected chat stays in the input for retry");
        })
    });
}

#[gpui_kit::test]
fn cooldown_keeps_one_deadline_during_further_events(cx: &mut TestAppContext) {
    let (viewer, cx) = fixture(cx);
    let due = cx.update(|_, cx| {
        viewer.update(cx, |v, cx| {
            v.rail = Rail::Tourneys;
            v.refresh_rail(true, cx);
            v.tourney_changed();
            v.refresh_tourneys();
            let due = v.session.reads.td_refresh.unwrap().due;
            assert_eq!(due, v.session.reads.sent["tourneys"] + super::reads::READ_COOLDOWN);
            due
        })
    });
    for _ in 0..4 {
        cx.update(|_, cx| {
            viewer.update(cx, |v, cx| v.on_message(lines(&[".tourney /td: ending round 2 of tournament 1"]), cx))
        });
        cx.cx.executor().advance_clock(std::time::Duration::from_secs(1));
        settle(cx);
        cx.update(|_, cx| {
            let v = viewer.read(cx);
            assert_eq!(v.session.reads.td_refresh.unwrap().due, due);
            assert_eq!(sent(v, "tell /td tourneys"), 1);
        });
    }
}

#[gpui_kit::test]
fn os_return_refetches_the_open_rail_and_games_sheet(cx: &mut TestAppContext) {
    let (viewer, cx) = fixture(cx);
    cx.update(|_, cx| {
        viewer.update(cx, |v, cx| {
            v.rail = Rail::Users;
            v.open_games("alice".into(), cx);
            v.refresh_rail(false, cx);
            v.on_message(vec![": - /os 1".into()], cx);
            v.on_message(vec![": + /os 1".into()], cx);
            let sent = |cmd: &str| v.log.iter().flat_map(|b| &b.lines).filter(|line| *line == cmd).count();
            assert_eq!(sent("> tell /os who 8 a"), 2, "the Users rail is listed again");
            assert_eq!(sent("> tell /os history alice"), 2, "the open Games sheet is filled again");
        })
    });
}

#[gpui_kit::test]
fn disconnect_forgets_the_sheet_and_back_history(cx: &mut TestAppContext) {
    let (viewer, cx) = fixture(cx);
    cx.update(|_, cx| {
        viewer.update(cx, |v, cx| {
            v.open_games("alice".into(), cx);
            v.open_tourney("12".into(), cx);
            assert_eq!(v.back.len(), 1);
            v.disconnect(cx);
            assert!(v.sheet.is_none());
            assert!(v.back.is_empty());
        })
    });
}

#[gpui_kit::test]
fn enter_in_the_password_field_connects(cx: &mut TestAppContext) {
    let (viewer, cx) = fixture(cx);
    cx.update(|window, cx| {
        viewer.update(cx, |v, cx| {
            v.session.screen = Screen::Login;
            v.form.port.update(cx, |s, cx| s.set_value("bad", window, cx));
            cx.notify();
        })
    });
    settle(cx);
    cx.update(|window, cx| {
        let focus = viewer.read(cx).form.password.focus_handle(cx);
        window.focus(&focus, cx);
        window.press("enter", cx);
    });
    settle(cx);
    assert_eq!(viewer.read_with(cx, |v, _| v.form.error.clone()).as_deref(), Some("bad port 'bad'"));
}

#[gpui_kit::test]
fn only_a_completed_login_is_kept_in_the_history(cx: &mut TestAppContext) {
    let (viewer, cx) = fixture(cx);
    cx.update(|_, cx| {
        viewer.update(cx, |v, cx| {
            let Screen::Connected(mut conn) = std::mem::replace(&mut v.session.screen, Screen::Login) else {
                unreachable!()
            };
            conn.params = Some(net::LoginParams {
                host: "typo.net".into(),
                port: 5000,
                login: "me".into(),
                password: String::new(),
                keepalive: net::KEEPALIVE,
            });
            v.session.screen = Screen::Connecting(conn);
            let hosts = |v: &Viewer| v.form.history.servers().iter().map(|e| e.host.clone()).collect::<Vec<_>>();
            assert!(hosts(v).is_empty(), "connecting alone records nothing");
            v.on_net(net::NetEvent::LoggedIn, cx);
            assert_eq!(hosts(v), ["typo.net"]);
        })
    });
}

#[gpui_kit::test]
fn the_tick_redraws_only_while_something_counts_down(cx: &mut TestAppContext) {
    let (viewer, cx) = fixture(cx);
    cx.update(|_, cx| {
        viewer.update(cx, |v, _| {
            v.rail = Rail::Users;
            assert!(v.has_countdown(), "the live .12 clock runs");
            v.session.lobby.games.get_mut(".12").unwrap().phase =
                crate::game::Phase::Over(crate::proto::Outcome::Aborted);
            assert!(!v.has_countdown(), "nothing ticks while browsing ended games");
            v.rail = Rail::Tourneys;
            assert!(v.has_countdown(), "tournament start times count down");
        })
    });
}

#[gpui_kit::test]
fn the_child_copy_button_copies_the_current_record(cx: &mut TestAppContext) {
    let (viewer, cx) = fixture(cx);
    cx.update(|_, cx| viewer.update(cx, |v, cx| v.watch_in_window(".12", cx)));
    settle(cx);
    let mut child = child_context(&viewer, ".12", cx);
    let button = child.debug_bounds("child-copy").unwrap();
    child.simulate_click(button.center(), Default::default());
    let expected = cx.update(|_, cx| viewer.read(cx).session.lobby.games[".12"].to_ggf());
    assert_eq!(child.read_from_clipboard().and_then(|c| c.text()), Some(expected));
}

#[gpui_kit::test]
fn the_elapsed_time_stays_inside_its_synchro_player_card(cx: &mut TestAppContext) {
    let (viewer, cx) = fixture(cx);
    add_synchro_pair(&viewer, cx);
    cx.update(|_, cx| {
        viewer.update(cx, |v, cx| {
            let Some((_, lobby)) = v.session.connected_mut() else { unreachable!() };
            lobby.apply(
                OsMsg::Update { id: ".20.0".into(), token: crate::othello::MoveToken::parse("e6//3").unwrap() },
                Instant::now(),
            );
            cx.notify();
        })
    });
    open_synchro(&viewer, cx);
    for width in [1240., 1000.] {
        cx.simulate_resize(size(px(width), px(920.)));
        settle(cx);
        let elapsed = cx.debug_bounds("player-elapsed-.20.0").expect("the running side shows its elapsed time");
        let side = ["player-cell-.20.0-black", "player-cell-.20.0-white"]
            .into_iter()
            .map(|cell| cx.debug_bounds(cell).unwrap())
            .find(|cell| cell.left() <= elapsed.left() && elapsed.left() < cell.right())
            .expect("the elapsed time starts inside a player card");
        assert!(elapsed.right() <= side.right(), "at width {width}, {elapsed:?} runs out of its card {side:?}");
    }
}

#[gpui_kit::test]
fn a_two_line_kibitz_fits_the_default_kibitz_pane(cx: &mut TestAppContext) {
    let (viewer, cx) = fixture(cx);
    cx.update(|_, cx| {
        viewer.update(cx, |v, cx| {
            let Some((_, lobby)) = v.session.connected_mut() else { unreachable!() };
            let game = lobby.games.get_mut(".12").unwrap();
            game.text = vec!["eve: what a squeeze on the south edge".into()];
            cx.notify();
        })
    });
    for (viewport, show_log) in [(size(px(1240.), px(920.)), true), (size(px(1236.), px(600.)), false)] {
        cx.update(|_, cx| viewer.update(cx, |v, _| v.show_log = show_log));
        cx.simulate_resize(viewport);
        settle(cx);
        let controls = cx.debug_bounds("kibitz-controls").unwrap();
        let text = cx.debug_bounds("kibitz-text").unwrap();
        cx.update(|window, _| {
            let line = window.find(("kibitz-line", 0usize)).bounds();
            assert!(line.size.height > px(20.), "the line {line:?} wraps, so it needs two rows");
            assert!(
                line.top() >= controls.bottom() && line.bottom() <= text.bottom(),
                "in {viewport:?}, the kibitz line {line:?} is clipped by its scroll area {text:?}"
            );
        });
    }
}

#[gpui_kit::test]
fn switching_views_keeps_the_rail_width(cx: &mut TestAppContext) {
    let (viewer, cx) = fixture(cx);
    add_synchro_pair(&viewer, cx);
    cx.simulate_resize(size(px(1400.), px(900.)));
    settle(cx);
    let focus = cx.debug_bounds("center").unwrap().left();
    open_synchro(&viewer, cx);
    let synchro = cx.debug_bounds("center").unwrap().left();
    assert_eq!(focus, synchro, "the rail keeps its width in the synchro view");
}

#[gpui_kit::test]
#[ignore]
fn bench_tourney_scroll(cx: &mut TestAppContext) {
    use crate::td::{Info, Outcome, Pairing, Standing, Style, TdMsg};
    let (viewer, cx) = fixture(cx);
    let n: usize = std::env::var("PLAYERS").map_or(30, |n| n.parse().unwrap());
    let players: Vec<String> = (0..n).map(|i| format!("player{i:02}")).collect();
    let mut pairings = Vec::new();
    for r in 0..n - 1 {
        for b in 0..n / 2 {
            let (x, y) = (if b == 0 { 0 } else { (r + b) % (n - 1) + 1 }, (r + n - 1 - b) % (n - 1) + 1);
            let result = if r + 2 < n {
                Outcome::Margin { black: (b as f64) - 7., ending: crate::td::Ending::Normal }
            } else {
                Outcome::Playing
            };
            pairings.push(Pairing {
                round: r as u32 + 1,
                board: b as u32 + 1,
                black: players[x].clone(),
                white: Some(players[y].clone()),
                result,
            });
        }
    }
    cx.update(|_, cx| {
        viewer.update(cx, |v, cx| {
            let t = &mut v.session.lobby.tourneys;
            let now = Instant::now();
            t.apply(
                TdMsg::Info(Info {
                    id: "1".into(),
                    played: n as u32 - 2,
                    rounds: n as u32 - 1,
                    style: Style::RoundRobin,
                    clock: "1:00//0:30".into(),
                    game_type: "8".into(),
                    players: players.clone(),
                    status: "round 29 is being played".into(),
                }),
                now,
            );
            t.apply(TdMsg::Schedule { id: "1".into(), pairings }, now);
            t.apply(
                TdMsg::Rankings {
                    id: "1".into(),
                    standings: players
                        .iter()
                        .map(|p| Standing {
                            points: 1.,
                            wins: 1,
                            draws: 0,
                            losses: 0,
                            discs: 0.,
                            login: p.clone(),
                            tiebreak: 0.,
                        })
                        .collect(),
                },
                now,
            );
            v.open_tourney("1".into(), cx);
        })
    });
    settle(cx);
    let position = point(px(620.), px(400.));
    cx.simulate_mouse_move(position, None, Default::default());
    let frames = 40;
    let start = std::time::Instant::now();
    for i in 0..frames {
        let dy = if i < frames / 2 { -40. } else { 40. };
        cx.update(|window, cx| {
            window.dispatch_event(
                ScrollWheelEvent { position, delta: ScrollDelta::Pixels(point(px(0.), px(dy))), ..Default::default() }
                    .to_platform_input(),
                cx,
            );
            window.render_frame(cx);
        });
    }
    eprintln!("BENCH tourney scroll: {:.2} ms/frame", start.elapsed().as_secs_f64() * 1000. / frames as f64);
}

#[gpui_kit::test]
fn a_finished_tournament_reopens_from_its_loaded_page(cx: &mut TestAppContext) {
    let (viewer, cx) = fixture(cx);
    cx.update(|_, cx| {
        viewer.update(cx, |v, cx| {
            let finger = |status: &str| {
                lines(&[
                    "/td: finger: 4",
                    "t me finger: ",
                    "id        : 4",
                    "joining   : closed",
                    "director  : root",
                    "rounds    : 1/1",
                    "style     : rrobin",
                    "clock     : 1:00//0:30",
                    "type      : 8",
                    "players   : 2 alice bob",
                    status,
                ])
            };
            let sent = |v: &Viewer| v.log.iter().flat_map(|b| &b.lines).filter(|l| *l == "> tell /td finger 4").count();
            v.on_message(finger("tournament has finished"), cx);
            v.session.lobby.tourneys.schedules.insert("4".into(), Vec::new());
            v.open_tourney("4".into(), cx);
            assert_eq!(sent(v), 1, "without standings the page is not loaded yet");
            v.session.lobby.tourneys.standings.insert("4".into(), Vec::new());
            v.session.reads.sent.clear();
            v.open_tourney("4".into(), cx);
            assert_eq!(sent(v), 1, "a finished tournament's loaded page no longer changes");
            v.on_message(finger("round 1 running"), cx);
            v.open_tourney("4".into(), cx);
            assert_eq!(sent(v), 2, "a running tournament is asked for afresh");
        })
    });
}

#[gpui_kit::test]
fn engine_logs_join_both_players_channels_and_reports_stay_out_of_the_chat(cx: &mut TestAppContext) {
    let (viewer, cx) = fixture(cx);
    let sent = |v: &Viewer, cmd: &str| v.log.iter().flat_map(|b| &b.lines).any(|line| line == cmd);
    let button = cx.debug_bounds("search-follow").expect("the focused game offers its engine logs");
    cx.simulate_click(button.center(), Default::default());
    settle(cx);
    cx.update(|_, cx| {
        viewer.update(cx, |v, cx| {
            assert!(sent(v, "> chann + .alice .bob"));
            v.on_message(
                vec![
                    ".bob bob: ------------".into(),
                    "bob plays C5 in game .12 using 4 threads ;".into(),
                    "score == 1 at 20 ; PV: C5 f6 ;".into(),
                ],
                cx,
            );
            let c5 = crate::othello::Move::parse("c5").unwrap();
            let report = v.session.lobby.games[".12"].search(c5).expect("the report is kept with its game");
            assert_eq!(report.pv.as_deref(), Some("C5 f6"));
            let Screen::Connected(conn) = &v.session.screen else { unreachable!() };
            assert!(
                conn.chat.conversations.iter().flat_map(|c| &c.lines).all(|said| !said
                    .line
                    .lines
                    .iter()
                    .any(|l| l.contains("plays"))),
                "a report is not chat"
            );
        })
    });
    settle(cx);
    assert!(cx.debug_bounds("search").is_some_and(|s| s.size.height > button.size.height * 3.), "the report is shown");
    let button = cx.debug_bounds("search-follow").unwrap();
    cx.simulate_click(button.center(), Default::default());
    settle(cx);
    cx.update(|_, cx| assert!(sent(viewer.read(cx), "> chann - .alice .bob")));
}
