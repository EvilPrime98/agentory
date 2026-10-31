//! Custom min/max/close for the frameless window (win32/linux). macOS keeps its native traffic
//! lights (`traffic_light_position` in the window options), so nothing is drawn there.

use gpui::{
    Action as _, InteractiveElement, IntoElement, ParentElement, Rgba, Styled, Window,
    WindowControlArea, div, px,
};

#[cfg(not(target_os = "windows"))]
use gpui::StatefulInteractiveElement as _;

use crate::app::RequestClose;
use crate::ui::icons::{Icon, icon};
use crate::ui::theme;

pub const HEIGHT_PX: f32 = 48.0;

fn button(
    id: &'static str,
    glyph: Icon,
    area: WindowControlArea,
    hover_bg: Rgba,
    on_click: impl Fn(&mut Window, &mut gpui::App) + 'static,
) -> impl IntoElement {
    // On Windows the OS hit-tests `window_control_area` and performs the action itself; an extra
    // click handler would double-fire. Elsewhere (Linux client decorations) we handle clicks.
    let _ = &on_click;
    let el = div()
        .id(id)
        .window_control_area(area)
        .flex()
        .items_center()
        .justify_center()
        .w(px(46.0))
        .h(px(HEIGHT_PX))
        .text_color(theme::text_muted())
        .hover(move |s| s.bg(hover_bg).text_color(theme::text()))
        .child(icon(glyph, px(16.0)).text_color(theme::text_muted()));
    #[cfg(not(target_os = "windows"))]
    let el = el.on_click(move |_, window, cx| on_click(window, cx));
    el
}

pub fn window_controls(window: &Window) -> Option<impl IntoElement> {
    if cfg!(target_os = "macos") {
        return None;
    }
    let maximized = window.is_maximized();
    Some(
        div()
            .flex()
            .flex_none()
            .child(button(
                "win-min",
                Icon::WinMinimize,
                WindowControlArea::Min,
                theme::hover(),
                |w, _| w.minimize_window(),
            ))
            .child(button(
                "win-max",
                if maximized {
                    Icon::WinRestore
                } else {
                    Icon::WinMaximize
                },
                WindowControlArea::Max,
                theme::hover(),
                |w, _| w.zoom_window(),
            ))
            .child(button(
                "win-close",
                Icon::Close,
                WindowControlArea::Close,
                gpui::rgb(0xe81123),
                // Through the root's close guard (running sessions ask first).
                |w, cx| w.dispatch_action(RequestClose.boxed_clone(), cx),
            )),
    )
}
