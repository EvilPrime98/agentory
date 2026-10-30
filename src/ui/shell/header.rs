//! Header: burger, logo → home, drag region, usage pill, skills + settings toggles, window controls.

use gpui::{
    Context, Entity, InteractiveElement, IntoElement, ParentElement, Rgba, StatefulInteractiveElement,
    Styled, Window, WindowControlArea, div, px,
};

use crate::app::AppRoot;
use crate::route::Route;
use crate::ui::icons::{Icon, icon};
use crate::ui::shell::usage_pill::UsagePill;
use crate::ui::shell::window_controls::window_controls;
use crate::ui::theme;

/// The Agentory mark: rounded square with an accent border and "A" (drawn natively because GPUI's
/// `svg()` cannot render text).
pub fn logo(size: f32) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .justify_center()
        .size(px(size))
        .rounded(px(size * 0.28))
        .border_1()
        .border_color(theme::accent())
        .bg(gpui::rgb(0x111111))
        .text_color(theme::accent())
        .text_size(px(size * 0.5))
        .font_weight(gpui::FontWeight::BOLD)
        .child("A")
}

fn header_button(id: &'static str, glyph: Icon, active: bool) -> gpui::Stateful<gpui::Div> {
    let color: Rgba = if active {
        theme::accent()
    } else {
        theme::text()
    };
    div()
        .id(id)
        .flex()
        .items_center()
        .justify_center()
        .size(px(34.0))
        .rounded(px(8.0))
        .cursor_pointer()
        .hover(|s| s.bg(theme::hover()))
        .child(icon(glyph, px(20.0)).text_color(color))
}

pub fn header(
    window: &Window,
    usage_pill: Entity<UsagePill>,
    skills_open: bool,
    settings_open: bool,
    cx: &mut Context<AppRoot>,
) -> impl IntoElement {
    div()
        .flex()
        .flex_none()
        .items_center()
        .h(theme::header_height())
        .w_full()
        .bg(theme::bg_chrome())
        .border_b_1()
        .border_color(theme::border_subtle())
        .child(
            header_button("burger", Icon::Burger, false)
                .ml(px(8.0))
                .on_click(cx.listener(|this, _, _, cx| this.toggle_sidebar(cx))),
        )
        .child(
            div()
                .id("logo")
                .flex()
                .items_center()
                .gap(px(10.0))
                .ml(px(8.0))
                .cursor_pointer()
                .on_click(
                    cx.listener(|this, _, window, cx| this.navigate(Route::home(), window, cx)),
                )
                .child(logo(28.0))
                .child(
                    div()
                        .text_color(theme::text())
                        .text_size(px(15.0))
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .child("Agentory"),
                ),
        )
        // Drag region: everything between the title and the controls moves the window.
        .child(
            div()
                .flex_1()
                .h_full()
                .window_control_area(WindowControlArea::Drag),
        )
        .child(usage_pill)
        .child(
            header_button("skills-toggle", Icon::Puzzle, skills_open)
                .on_click(cx.listener(|this, _, _, cx| this.toggle_skills(cx))),
        )
        .child(
            header_button("settings", Icon::Gear, settings_open)
                .mr(px(8.0))
                .on_click(cx.listener(|this, _, window, cx| {
                    let target = if this.route() == &Route::Settings {
                        Route::home()
                    } else {
                        Route::Settings
                    };
                    this.navigate(target, window, cx)
                })),
        )
        .children(window_controls(window))
}
