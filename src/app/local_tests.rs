//! Opt-in checks against the local GGS. Never uses saved logins or a public server.
use super::*;
use std::time::Duration;

struct LocalConnection {
    link: net::Link,
    events: async_channel::Receiver<net::NetEvent>,
}

impl Drop for LocalConnection {
    fn drop(&mut self) {
        self.link.close();
    }
}

fn connect(viewer: &Entity<Viewer>, cx: &mut VisualTestContext, params: net::LoginParams) -> LocalConnection {
    let (tx, events) = async_channel::bounded(64);
    let link = net::connect(params.clone(), tx);
    cx.update(|_, cx| {
        viewer.update(cx, |v, cx| {
            v.disconnect(cx);
            v.session.lobby.me = params.login.clone();
            v.session.screen = Screen::Connecting(Conn {
                link: link.clone(),
                label: "local integration test".into(),
                chat: Chat::new(None, params.login.clone()),
                params: Some(params),
                attempt: 0,
                logged_in_at: None,
                _pump: Task::ready(()),
            });
        })
    });
    LocalConnection { link, events }
}

fn wait_for(
    connection: &LocalConnection,
    viewer: &Entity<Viewer>,
    cx: &mut VisualTestContext,
    label: &str,
    ready: impl Fn(&Viewer) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        // Real socket threads must not wake GPUI's deterministic test scheduler.
        // Dispatch on the test thread through the same on_net entry point as the app pump.
        while let Ok(event) = connection.events.try_recv() {
            cx.update(|_, cx| viewer.update(cx, |v, cx| v.on_net(event, cx)));
        }
        settle(cx);
        if cx.update(|_, cx| ready(viewer.read(cx))) {
            return;
        }
        assert!(Instant::now() < deadline, "local GGS timed out waiting for {label}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[gpui_kit::test]
#[ignore = "requires local GGS with /os and /td on 127.0.0.1:5000"]
fn local_ggs_login_tournaments_and_reconnect(cx: &mut TestAppContext) {
    let (viewer, cx) = fixture(cx);
    let params = net::LoginParams {
        host: "127.0.0.1".into(),
        port: 5000,
        login: format!("v{:07}", std::process::id() % 10_000_000),
        password: String::new(),
        keepalive: net::KEEPALIVE,
    };
    let connection = connect(&viewer, cx, params.clone());
    wait_for(&connection, &viewer, cx, "login and /os", |v| {
        v.session.connected().is_some() && v.session.lobby.os == crate::lobby::OsState::Present
    });
    click("rail-tab-3", cx);
    wait_for(&connection, &viewer, cx, "tournament list", |v| v.session.lobby.tourneys.list.is_some());
    let id = cx.update(|_, cx| {
        let v = viewer.read(cx);
        let list = v.session.lobby.tourneys.rows();
        list.iter()
            .rev()
            .find(|t| t.start == crate::td::Start::Done)
            .expect("seed a completed local tournament before running this check")
            .id
            .clone()
    });
    cx.update(|_, cx| viewer.update(cx, |v, cx| v.open_tourney(id.clone(), cx)));
    wait_for(&connection, &viewer, cx, "tournament schedule", |v| v.session.lobby.tourneys.schedules.contains_key(&id));
    cx.update(|_, cx| {
        viewer.update(cx, |v, cx| {
            v.disconnect(cx);
            assert!(matches!(v.session.screen, Screen::Login));
        })
    });
    drop(connection);
    // A fresh connection restores the still-visible tournament without another page click.
    let connection = connect(&viewer, cx, params);
    wait_for(&connection, &viewer, cx, "reconnected tournament", |v| {
        v.session.lobby.tourneys.schedules.contains_key(&id)
    });
    cx.update(|_, cx| viewer.update(cx, |v, cx| v.disconnect(cx)));
    drop(connection);
}
