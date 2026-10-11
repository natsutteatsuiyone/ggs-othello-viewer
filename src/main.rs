// Release builds open no console window next to the app.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod chat;
mod clock;
mod game;
mod ggf;
mod history;
mod lobby;
mod net;
mod othello;
mod proto;
mod store;
mod td;
mod ui;

use gpui_kit::*;

use std::path::PathBuf;

/// Where the app keeps `name`: the user's data folder (`%APPDATA%` on Windows, `~/Library/Application Support`
/// on macOS, `~/.local/share` on Linux) under `ggs-othello-viewer`, or the working directory if there is none.
pub fn data_file(name: &str) -> PathBuf {
    #[cfg(test)]
    let dir = Some(std::env::temp_dir().join(format!("ggs-othello-viewer-test-{}", std::process::id())));
    #[cfg(not(test))]
    let dir = dirs::data_dir().map(|d| d.join("ggs-othello-viewer"));
    dir.filter(|d| std::fs::create_dir_all(d).is_ok()).map_or_else(|| name.into(), |d| d.join(name))
}

/// Prints `message` to stderr; a release build on Windows first attaches to the console it was started from.
fn report(message: &str) {
    #[cfg(all(windows, not(debug_assertions)))]
    {
        unsafe extern "system" {
            fn AttachConsole(process_id: u32) -> i32;
        }
        const ATTACH_PARENT_PROCESS: u32 = u32::MAX;
        // SAFETY: takes no pointers; it fails harmlessly when there is no parent console.
        unsafe { AttachConsole(ATTACH_PARENT_PROCESS) };
    }
    eprintln!("{message}");
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let launch = match net::login_from_args(&args) {
        Ok(Some(params)) => app::Launch::Connect(params),
        Ok(None) if args.iter().any(|a| a == "--demo") => app::Launch::Demo,
        Ok(None) => app::Launch::Login,
        Err(e) => {
            report(&format!(
                "{e}\nusage: ggs-othello-viewer [--demo | --connect HOST:PORT --login NAME [--password PW]]"
            ));
            std::process::exit(2);
        }
    };
    gpui_kit::application().with_assets(gpui_kit::assets::AllAssets).run(move |cx| {
        gpui_kit::init(cx);
        let db = store::open();
        let theme = db.as_ref().map_or(ui::ThemeChoice::System, ui::ThemeChoice::load);
        ui::install(theme.is_light(cx.window_appearance()), cx);
        let layout = db.as_ref().map_or_else(|_| ui::Layout::default(), ui::Layout::load);
        // Clamped to the screen's work area, so a small screen still gets the whole window.
        let bounds = Bounds::centered(None, size(px(layout.width), px(layout.height)), cx);
        let options = WindowOptions {
            window_bounds: Some(if layout.maximized {
                WindowBounds::Maximized(bounds)
            } else {
                WindowBounds::Windowed(bounds)
            }),
            titlebar: Some(TitlebarOptions { title: Some("GGS Othello Viewer".into()), ..Default::default() }),
            window_min_size: Some(size(px(ui::SIDE_MINIMA.iter().sum()), px(520.))),
            ..Default::default()
        };
        match gpui_kit::open_window(options, cx, |window, cx| {
            cx.new(|cx| app::Viewer::new(launch, theme, db, window, cx))
        }) {
            Ok((main_window, _)) => {
                cx.on_window_closed(move |cx, closed| {
                    if closed == main_window.window_id() {
                        cx.quit();
                    }
                })
                .detach();
            }
            Err(e) => {
                report(&format!("failed to open window: {e}"));
                std::process::exit(1);
            }
        }
    });
}
