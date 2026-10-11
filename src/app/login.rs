use super::*;

pub(super) struct LoginForm {
    pub(super) host: Entity<InputState>,
    pub(super) port: Entity<InputState>,
    pub(super) login: Entity<InputState>,
    pub(super) password: Entity<InputState>,
    pub(super) remember_password: bool,
    pub(super) restored_password: bool,
    pub(super) credential_generation: u64,
    pub(super) credentials: credentials::Credentials,
    pub(super) credential_task: Option<Task<()>>,
    pub(super) history: History,
    pub(super) error: Option<String>,
}

impl Viewer {
    pub(super) fn connect(&mut self, cx: &mut Context<Self>) {
        let Ok(port) = text(&self.form.port, cx).parse::<u16>() else {
            self.fail_login(format!("bad port '{}'", text(&self.form.port, cx)));
            cx.notify();
            return;
        };
        let params = LoginParams {
            host: text(&self.form.host, cx),
            port,
            login: text(&self.form.login, cx),
            password: self.form.password.read(cx).value().to_string(),
            keepalive: net::KEEPALIVE,
        };
        self.save_login_password(&params, cx);
        self.open(params, cx);
    }

    pub(super) fn save_login_password(&mut self, params: &LoginParams, cx: &mut Context<Self>) {
        if params.validate().is_ok() {
            self.form.credential_generation += 1;
            if let Some(key) = self.credential_key(cx) {
                self.persist_password(
                    key,
                    self.form.remember_password.then(|| (params.login.clone(), params.password.as_bytes().to_vec())),
                    cx,
                );
            }
        }
    }

    pub(super) fn fail_login(&mut self, error: String) {
        note(&mut self.log, error.clone());
        self.form.error = Some(error);
    }

    pub(super) fn credential_key(&self, cx: &App) -> Option<String> {
        let port = text(&self.form.port, cx).parse::<u16>().ok()?;
        let host = text(&self.form.host, cx).to_ascii_lowercase();
        let login = text(&self.form.login, cx);
        (!host.is_empty() && !login.is_empty())
            .then(|| format!("ggs-othello-viewer:{}:{host}:{port}:{}:{login}", host.len(), login.len()))
    }

    pub(super) fn restore_password(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.form.credential_generation += 1;
        self.form.remember_password = false;
        if self.form.restored_password {
            self.form.password.update(cx, |s, cx| s.set_value("", window, cx));
            self.form.restored_password = false;
        }
        if !self.form.password.read(cx).value().is_empty() {
            return;
        }
        let Some(key) = self.credential_key(cx) else { return };
        let generation = self.form.credential_generation;
        let previous = self.form.credential_task.take();
        let store = self.form.credentials.clone();
        self.form.credential_task = Some(cx.spawn_in(window, async move |this, cx| {
            if let Some(previous) = previous {
                previous.await;
            }
            if !this.read_with(cx, |v, _| v.password_saved(&key)).unwrap_or(false) {
                return;
            }
            let Ok(read) = cx.update(|_, cx| store.read(&key, cx)) else { return };
            let result = read.await;
            if let Some(entity) = this.upgrade() {
                let _ = cx.update_window_entity(&entity, |v, window, cx| {
                    if matches!(result, Ok(None)) {
                        v.mark_saved(&key, false);
                    }
                    if v.form.credential_generation != generation
                        || v.credential_key(cx).as_ref() != Some(&key)
                        || !matches!(v.session.screen, Screen::Login)
                        || !v.form.password.read(cx).value().is_empty()
                    {
                        return;
                    }
                    match result {
                        Ok(Some((login, password))) if login == text(&v.form.login, cx) => {
                            if let Ok(password) = String::from_utf8(password) {
                                v.form.password.update(cx, |s, cx| s.set_value(password, window, cx));
                                v.form.remember_password = true;
                                v.form.restored_password = true;
                            }
                        }
                        Err(()) => v.fail_login("Could not read saved password.".into()),
                        _ => {}
                    }
                    cx.notify();
                });
            }
        }));
    }

    pub(super) fn persist_password(&mut self, key: String, value: Option<(String, Vec<u8>)>, cx: &mut Context<Self>) {
        let forget = value.is_none().then(|| key.clone());
        let previous = self.form.credential_task.take();
        let store = self.form.credentials.clone();
        self.form.credential_task = Some(cx.spawn(async move |this, cx| {
            if let Some(previous) = previous {
                previous.await;
            }
            // Marked before the write, so a crash can leave a mark without a password but never a password without one.
            let saved = this.update(cx, |v, cx| match value {
                Some(_) if !v.mark_saved(&key, true) => {
                    note(&mut v.log, "Could not update saved password.");
                    cx.notify();
                    false
                }
                Some(_) => true,
                None => v.password_saved(&key),
            });
            if !saved.unwrap_or(false) {
                return;
            }
            let result = if let Some((login, password)) = value {
                cx.update(|cx| store.write(&key, &login, &password, cx)).await
            } else {
                let read = cx.update(|cx| store.read(&key, cx));
                match read.await {
                    Ok(None) => Ok(()),
                    Err(()) => Err(()),
                    Ok(Some(_)) => cx.update(|cx| store.delete(&key, cx)).await,
                }
            };
            let _ = this.update(cx, |v, cx| {
                if result.is_err() || forget.is_some_and(|key| !v.mark_saved(&key, false)) {
                    note(&mut v.log, "Could not update saved password.");
                    cx.notify();
                }
            });
        }));
    }

    /// Whether this app saved a password for `key`. The credential store is only touched for those keys, as on some
    /// systems any access asks the user for permission. Read from `ggs.db` each time, as other instances share it.
    pub(super) fn password_saved(&self, key: &str) -> bool {
        self.db.as_ref().is_some_and(|db| crate::store::password_saved(db, key).unwrap_or(false))
    }

    /// False when `ggs.db` is missing or the write fails. A password is only saved after its mark is.
    pub(super) fn mark_saved(&self, key: &str, saved: bool) -> bool {
        self.db.as_ref().is_some_and(|db| crate::store::mark_password_saved(db, key, saved).is_ok())
    }

    pub(super) fn remember_password(&mut self, checked: bool, cx: &mut Context<Self>) {
        self.form.credential_generation += 1;
        self.form.remember_password = checked;
        if !checked && let Some(key) = self.credential_key(cx) {
            self.persist_password(key, None, cx);
        }
        cx.notify();
    }

    pub(super) fn fill_login(&mut self, e: Entry, window: &mut Window, cx: &mut Context<Self>) {
        for (state, value) in [
            (&self.form.host, e.host),
            (&self.form.port, e.port),
            (&self.form.login, e.login),
            (&self.form.password, String::new()),
        ] {
            state.update(cx, |s, cx| s.set_value(value, window, cx));
        }
        self.form.restored_password = false;
        self.restore_password(window, cx);
        cx.notify();
    }

    pub(super) fn render_login(&self, cx: &mut Context<Self>) -> Div {
        let row = |label: &'static str, state: &Entity<InputState>| {
            div().v_flex().gap_1().child(ui::header(label)).child(Input::new(state))
        };
        let (host, port) = (text(&self.form.host, cx), text(&self.form.port, cx));
        let weak = cx.weak_entity();
        // A typed field with a menu of past entries on its right, picking one fills the whole form.
        let combo = |label: &'static str, state: &Entity<InputState>, id: &'static str, picks: Vec<(String, Entry)>| {
            let input = Input::new(state);
            let input = if picks.is_empty() {
                input
            } else {
                let weak = weak.clone();
                input.suffix(
                    Button::new(id).ghost().xsmall().cursor_pointer().icon(IconName::ChevronDown).dropdown_menu(
                        move |mut menu, _, _| {
                            for (text, e) in picks.clone() {
                                let weak = weak.clone();
                                menu = menu.item(PopupMenuItem::new(text).on_click(move |_, window, cx| {
                                    let _ = weak.update(cx, |this, cx| this.fill_login(e.clone(), window, cx));
                                }));
                            }
                            menu
                        },
                    ),
                )
            };
            div().v_flex().gap_1().child(ui::header(label)).child(input)
        };
        let servers =
            self.form.history.servers().into_iter().map(|e| (format!("{}:{}", e.host, e.port), e.clone())).collect();
        let logins = self.form.history.logins(&host, &port).map(|e| (e.login.clone(), e.clone())).collect();
        div().size_full().flex().items_center().justify_center().child(
            ui::boxed()
                .gap_3()
                .w(px(ui::LOGIN_W))
                .p_5()
                .child(div().text_lg().font_weight(FontWeight::BOLD).child("Connect to GGS"))
                .child(combo("HOST", &self.form.host, "past-servers", servers))
                .child(row("PORT", &self.form.port))
                .child(combo("LOGIN", &self.form.login, "past-logins", logins))
                .child(row("PASSWORD", &self.form.password))
                .child(
                    Checkbox::new("remember-password")
                        .cursor_pointer()
                        .label("Remember password")
                        .checked(self.form.remember_password)
                        .debug_selector(|| "remember-password".into())
                        .on_click(cx.listener(|this, checked: &bool, _, cx| this.remember_password(*checked, cx))),
                )
                .child(
                    Button::new("ggs-site")
                        .ghost()
                        .xsmall()
                        .cursor_pointer()
                        .icon(IconName::ExternalLink)
                        .label("About GGS")
                        .on_click(|_, _, cx| cx.open_url("https://skatgame.net/mburo/ggs/")),
                )
                .child(
                    Button::new("connect")
                        .primary()
                        .cursor_pointer()
                        .label("Connect")
                        .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.connect(cx))),
                )
                .when_some(self.form.error.clone(), |d, e| {
                    d.child(
                        div()
                            .debug_selector(|| "login-error".into())
                            .px_3()
                            .py_2()
                            .rounded(px(ui::RADIUS_ROW))
                            .border_1()
                            .border_color(ui::error())
                            .text_sm()
                            .text_color(ui::error())
                            .child(e),
                    )
                }),
        )
    }
}
