// No console window behind the GUI in release builds on Windows.
#![cfg_attr(all(not(debug_assertions), windows), windows_subsystem = "windows")]

mod app;
mod assets;
mod backend;
mod i18n;
mod model;
mod platform;
mod route;
mod runtime;
mod state;
mod ui;

use gpui::{
    App, Bounds, TitlebarOptions, WindowBounds, WindowDecorations, WindowOptions, point,
    prelude::*, px, size,
};

use crate::app::AppRoot;
use crate::assets::Assets;
use crate::ui::theme;

fn main() {
    // Held until `main` returns so buffered log lines are flushed.
    let _log_guard = platform::logging::init();

    gpui_ce_platform::application()
        .with_assets(Assets)
        .run(|cx: &mut App| {
            theme::register_fonts(cx);
            app::bind_keys(cx);

            // Closing the last window quits (dropping the root kills the PTY children).
            cx.on_window_closed(|cx, _| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();

            // 1240×950, min 900×600, frameless on win/linux with custom controls (macOS: inset
            // titlebar with native traffic lights).
            let bounds = Bounds::centered(None, size(px(1240.0), px(950.0)), cx);
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_min_size: Some(size(px(900.0), px(600.0))),
                titlebar: Some(TitlebarOptions {
                    title: Some("Agentory".into()),
                    appears_transparent: true,
                    traffic_light_position: Some(point(px(12.0), px(20.0))),
                }),
                window_decorations: Some(WindowDecorations::Client),
                app_id: Some("agentory".into()),
                ..Default::default()
            };
            cx.open_window(options, |window, cx| cx.new(|cx| AppRoot::new(window, cx)))
                .unwrap();
            cx.activate(true);
        });
}
