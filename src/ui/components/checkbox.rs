//! Checkbox: a 16 px accent box with a tick, plus its label.

use gpui::{
    App, ElementId, InteractiveElement, IntoElement, ParentElement, SharedString,
    StatefulInteractiveElement, Styled, Window, div, px,
};

use crate::ui::theme;

pub fn checkbox(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    checked: bool,
    on_toggle: impl Fn(bool, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let mut tick = div()
        .flex()
        .items_center()
        .justify_center()
        .size(px(16.0))
        .flex_none()
        .rounded(px(4.0))
        .border_1()
        .border_color(if checked {
            theme::accent()
        } else {
            gpui::rgba(0xffffff4d)
        });
    if checked {
        tick = tick
            .bg(theme::accent())
            .text_size(px(12.0))
            .text_color(theme::on_accent())
            .child("✓");
    }
    div()
        .id(id)
        .flex()
        .items_start()
        .gap(px(8.0))
        .cursor_pointer()
        .text_size(px(13.0))
        .text_color(gpui::rgb(0xe6e6e6))
        .on_click(move |_, window, cx| on_toggle(!checked, window, cx))
        .child(tick.mt(px(1.0)))
        .child(div().flex_1().min_w_0().child(label.into()))
}
