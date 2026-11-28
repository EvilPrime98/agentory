use std::rc::Rc;

use gpui::{
    App, Div, ElementId, InteractiveElement, ParentElement, SharedString, Stateful,
    StatefulInteractiveElement, Styled, Window, deferred, div, prelude::FluentBuilder, px,
};

use crate::ui::components::tooltip::tooltip;
use crate::ui::icons::{Icon, icon};
use crate::ui::theme;

const ROW_H: f32 = 28.0;
const ICON_POPUP_W: f32 = 180.0;

pub fn dropdown(
    id: impl Into<SharedString>,
    labels: Vec<SharedString>,
    selected: usize,
    open: bool,
    on_toggle: impl Fn(bool, &mut Window, &mut App) + 'static,
    on_select: impl Fn(usize, &mut Window, &mut App) + 'static,
) -> gpui::Div {
    let id: SharedString = id.into();
    let on_toggle = Rc::new(on_toggle);
    let on_select = Rc::new(on_select);
    let current = labels.get(selected).cloned().unwrap_or_default();

    let toggle = on_toggle.clone();
    let field = div()
        .id(ElementId::from(id.clone()))
        .flex()
        .items_center()
        .justify_between()
        .gap(px(8.0))
        .h(px(32.0))
        .px(px(10.0))
        .rounded(px(4.0))
        .border_1()
        .border_color(theme::border_subtle())
        .bg(theme::bg_app())
        .text_size(px(13.0))
        .text_color(theme::text())
        .cursor_pointer()
        .hover(|s| s.border_color(theme::border_strong()))
        .on_click(move |_, window, cx| toggle(!open, window, cx))
        .child(div().flex_1().truncate().child(current))
        .child(icon(Icon::ChevronDown, px(12.0)).text_color(theme::text_muted()));

    let popup = open.then(|| {
        let list = popup_list(&id, labels, selected, on_toggle, on_select)
            .top(px(36.0))
            .left_0()
            .right_0();
        deferred(list).with_priority(2)
    });

    div().relative().child(field).children(popup)
}

#[allow(clippy::too_many_arguments)]
pub fn icon_dropdown(
    id: impl Into<SharedString>,
    glyph: Icon,
    hint: impl Into<SharedString>,
    labels: Vec<SharedString>,
    selected: usize,
    open: bool,
    on_toggle: impl Fn(bool, &mut Window, &mut App) + 'static,
    on_select: impl Fn(usize, &mut Window, &mut App) + 'static,
) -> gpui::Div {
    let id: SharedString = id.into();
    let on_toggle = Rc::new(on_toggle);
    let hint: SharedString = hint.into();

    let toggle = on_toggle.clone();
    let trigger = div()
        .id(ElementId::from(id.clone()))
        .size(px(20.0))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(4.0))
        .cursor_pointer()
        .hover(|s| s.bg(theme::hover()))
        .on_click(move |_, window, cx| toggle(!open, window, cx))
        .when(!open, |s| s.tooltip(tooltip(hint)))
        .child(icon(glyph, px(12.0)).text_color(theme::text_muted()));

    let popup = open.then(|| {
        let list = popup_list(&id, labels, selected, on_toggle, Rc::new(on_select))
            .top(px(24.0))
            .right_0()
            .w(px(ICON_POPUP_W));
        deferred(list).with_priority(2)
    });

    div().relative().flex_none().child(trigger).children(popup)
}

fn popup_list(
    id: &SharedString,
    labels: Vec<SharedString>,
    selected: usize,
    on_toggle: Rc<impl Fn(bool, &mut Window, &mut App) + 'static>,
    on_select: Rc<impl Fn(usize, &mut Window, &mut App) + 'static>,
) -> Stateful<Div> {
    div()
        .id(ElementId::from(SharedString::from(format!("{id}-list"))))
        .occlude()
        .absolute()
        .max_h(px(ROW_H * 7.0))
        .overflow_y_scroll()
        .p(px(4.0))
        .flex()
        .flex_col()
        .rounded(px(8.0))
        .border_1()
        .border_color(theme::border_subtle())
        .bg(theme::bg_modal())
        .on_mouse_down_out(move |_, window, cx| on_toggle(false, window, cx))
        .children(labels.into_iter().enumerate().map(|(i, label)| {
            let on_select = on_select.clone();
            div()
                .id(ElementId::from(SharedString::from(format!("{id}-{i}"))))
                .flex()
                .items_center()
                .h(px(ROW_H))
                .px(px(8.0))
                .flex_none()
                .rounded(px(5.0))
                .text_size(px(12.5))
                .text_color(if i == selected {
                    theme::accent()
                } else {
                    theme::text()
                })
                .cursor_pointer()
                .hover(|s| s.bg(theme::hover()))
                .on_click(move |_, window, cx| on_select(i, window, cx))
                .child(label)
        }))
}
