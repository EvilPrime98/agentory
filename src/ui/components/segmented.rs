use std::rc::Rc;

use gpui::{
    App, ElementId, InteractiveElement, ParentElement, SharedString, Stateful,
    StatefulInteractiveElement, Styled, Window, div, px,
};

use crate::ui::theme;

pub fn segmented(
    id: impl Into<SharedString>,
    labels: Vec<SharedString>,
    selected: usize,
    on_change: impl Fn(usize, &mut Window, &mut App) + 'static,
) -> Stateful<gpui::Div> {
    let id: SharedString = id.into();
    let on_change = Rc::new(on_change);
    div()
        .id(ElementId::from(id.clone()))
        .flex()
        .items_center()
        .h(px(28.0))
        .p(px(2.0))
        .gap(px(2.0))
        .rounded(px(6.0))
        .border_1()
        .border_color(theme::border_subtle())
        .bg(theme::bg_app())
        .children(labels.into_iter().enumerate().map(|(i, label)| {
            let on_change = on_change.clone();
            let base = div()
                .id(ElementId::from(SharedString::from(format!("{id}-{i}"))))
                .flex()
                .flex_1()
                .items_center()
                .justify_center()
                .h_full()
                .px(px(10.0))
                .rounded(px(4.0))
                .text_size(px(12.0))
                .whitespace_nowrap()
                .cursor_pointer()
                .on_click(move |_, window, cx| on_change(i, window, cx))
                .child(label);
            if i == selected {
                base.bg(theme::accent())
                    .text_color(theme::on_accent())
                    .font_weight(gpui::FontWeight::SEMIBOLD)
            } else {
                base.text_color(theme::text_secondary())
                    .hover(|s| s.bg(theme::hover()).text_color(theme::text()))
            }
        }))
}
