use gpui::{Div, InteractiveElement, ParentElement, SharedString, Styled, Window, div, px};

use super::markdown::markdown;
use super::smooth_scroll::SmoothScroll;
use crate::ui::theme;

pub fn preview_pane(
    id: impl Into<gpui::ElementId>,
    text: &str,
    empty: SharedString,
    scroll: &SmoothScroll,
    window: &mut Window,
) -> Div {
    let body = if text.trim().is_empty() {
        div()
            .text_size(px(12.0))
            .text_color(theme::text_muted())
            .child(empty)
    } else {
        markdown(text)
    };
    div()
        .flex()
        .flex_col()
        .flex_1()
        .min_w_0()
        .rounded(px(6.0))
        .border_1()
        .border_color(theme::border_subtle())
        .bg(theme::bg_app())
        .child(
            scroll.attach(
                div()
                    .id(id)
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .p(px(16.0))
                    .child(body),
                window,
            ),
        )
}
