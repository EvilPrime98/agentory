use std::rc::Rc;

use gpui::{
    Anchor, AnyElement, App, ElementId, FocusHandle, InteractiveElement, IntoElement,
    ParentElement, Pixels, Point, SharedString, StatefulInteractiveElement, Styled, Window,
    anchored, deferred, div, prelude::FluentBuilder, px,
};

use crate::ui::icons::{Icon, icon};
use crate::ui::theme;

pub struct MenuItem {
    pub id: ElementId,
    pub icon: Icon,
    pub label: SharedString,
    pub disabled: bool,
    pub on_click: Rc<dyn Fn(&mut Window, &mut App)>,
}

pub fn context_menu(
    position: Point<Pixels>,
    title: SharedString,
    focus: &FocusHandle,
    items: Vec<MenuItem>,
    on_close: impl Fn(&mut Window, &mut App) + 'static,
) -> AnyElement {
    let on_close = Rc::new(on_close);
    let (out_close, key_close) = (on_close.clone(), on_close.clone());
    let menu = div()
        .id("context-menu")
        .track_focus(focus)
        .occlude()
        .w(px(224.0))
        .p(px(6.0))
        .flex()
        .flex_col()
        .rounded(px(10.0))
        .border_1()
        .border_color(theme::border_strong())
        .shadow_lg()
        .bg(theme::bg_menu())
        .text_color(theme::text())
        .on_mouse_down_out(move |_, window, cx| out_close(window, cx))
        .on_key_down(move |e, window, cx| {
            if e.keystroke.key == "escape" {
                key_close(window, cx);
            }
        })
        .child(
            div()
                .px(px(8.0))
                .pt(px(4.0))
                .pb(px(6.0))
                .mb(px(4.0))
                .border_b_1()
                .border_color(theme::border_subtle())
                .truncate()
                .text_size(px(11.0))
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(theme::text_muted())
                .child(title),
        )
        .children(items.into_iter().map(|item| {
            let close = on_close.clone();
            let on_click = item.on_click;
            div()
                .id(item.id)
                .flex()
                .items_center()
                .gap(px(8.0))
                .h(px(32.0))
                .px(px(8.0))
                .rounded(px(6.0))
                .text_size(px(13.0))
                .when(item.disabled, |s| s.opacity(0.4))
                .when(!item.disabled, |s| {
                    s.cursor_pointer()
                        .hover(|s| s.bg(theme::accent_soft()))
                        .on_click(move |_, window, cx| {
                            on_click(window, cx);
                            close(window, cx);
                        })
                })
                .child(icon(item.icon, px(14.0)).text_color(theme::text_muted()))
                .child(div().flex_1().child(item.label))
        }));
    deferred(
        anchored()
            .position(position)
            .anchor(Anchor::TopLeft)
            .snap_to_window_with_margin(px(8.0))
            .child(menu),
    )
    .with_priority(3)
    .into_any_element()
}
