//! Button: 34 px tall, 4 px radius, 13 px / 600.

use gpui::{
    App, ClickEvent, ElementId, InteractiveElement, ParentElement, SharedString, Stateful,
    StatefulInteractiveElement, Styled, Window, div, px,
};

use crate::ui::theme;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ButtonVariant {
    /// Outlined accent; the confirm button in dialogs (`classic`).
    Classic,
    Secondary,
    #[allow(dead_code)]
    Ghost,
}

pub fn button(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    variant: ButtonVariant,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Stateful<gpui::Div> {
    let base = div()
        .id(id)
        .flex()
        .items_center()
        .justify_center()
        .gap(px(6.0))
        .h(px(34.0))
        .min_w(px(96.0))
        .px(px(16.0))
        .rounded(px(4.0))
        .border_1()
        .border_color(gpui::transparent_black())
        .text_size(px(13.0))
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .whitespace_nowrap()
        .cursor_pointer()
        .on_click(on_click)
        .child(label.into());
    match variant {
        ButtonVariant::Classic => base
            .text_color(theme::accent())
            .border_2()
            .border_color(theme::accent())
            .hover(|s| s.bg(theme::accent()).text_color(theme::on_accent()))
            .active(|s| s.bg(theme::accent()).text_color(theme::on_accent())),
        ButtonVariant::Secondary => base
            .bg(theme::bg_panel())
            .border_color(theme::border_subtle())
            .text_color(theme::text())
            .hover(|s| s.bg(theme::btn_hover()).border_color(theme::border_strong()))
            .active(|s| s.bg(theme::btn_active())),
        ButtonVariant::Ghost => base
            .text_color(theme::text_secondary())
            .hover(|s| s.bg(theme::hover_soft()).text_color(theme::text()))
            .active(|s| s.bg(theme::pressed())),
    }
}

/// Compact text button for dense panels (26 px tall).
pub fn small_button(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Stateful<gpui::Div> {
    div()
        .id(id)
        .flex()
        .items_center()
        .justify_center()
        .h(px(26.0))
        .px(px(10.0))
        .rounded(px(4.0))
        .border_1()
        .border_color(theme::border_subtle())
        .text_size(px(12.0))
        .text_color(theme::text_secondary())
        .whitespace_nowrap()
        .cursor_pointer()
        .hover(|s| s.bg(theme::hover()).text_color(theme::text()))
        .on_click(on_click)
        .child(label.into())
}
