use gpui::{
    AnyView, App, AppContext, Context, IntoElement, ParentElement, Render, SharedString, Styled,
    Window, div, px,
};

use crate::ui::theme;

const CURSOR_CLEARANCE_X: f32 = 8.0;
const CURSOR_CLEARANCE_Y: f32 = 10.0;

struct Tooltip(SharedString);

impl Render for Tooltip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .pl(px(CURSOR_CLEARANCE_X))
            .pt(px(CURSOR_CLEARANCE_Y))
            .child(
                div()
                    .px(px(8.0))
                    .py(px(4.0))
                    .rounded(px(4.0))
                    .border_1()
                    .border_color(theme::border_subtle())
                    .bg(theme::bg_menu())
                    .font_family(theme::FONT)
                    .text_size(px(11.5))
                    .text_color(theme::text())
                    .child(self.0.clone()),
            )
    }
}

pub fn tooltip(label: impl Into<SharedString>) -> impl Fn(&mut Window, &mut App) -> AnyView {
    let label = label.into();
    move |_, cx| cx.new(|_| Tooltip(label.clone())).into()
}
