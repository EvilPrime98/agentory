//! `AppLoader`: full-window splash while the startup sequence runs. The spinner is a pulsing ring.

use std::time::Duration;

use gpui::{
    Animation, AnimationExt as _, InteractiveElement, IntoElement, ParentElement, SharedString,
    Styled, div, ease_in_out, px,
};

use crate::ui::shell::header::logo;
use crate::ui::theme;

pub fn app_loader(message: impl Into<SharedString>) -> impl IntoElement {
    div()
        .id("app-loader")
        .absolute()
        .inset_0()
        .occlude()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(14.0))
        .bg(theme::bg_chrome())
        .child(
            div().child(logo(88.0)).with_animation(
                "loader-pulse",
                Animation::new(Duration::from_millis(1800))
                    .repeat()
                    .with_easing(ease_in_out),
                |el, t| el.opacity(1.0 - 0.15 * (1.0 - (2.0 * t - 1.0).abs())),
            ),
        )
        .child(
            div()
                .flex()
                .text_size(px(22.0))
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(theme::text())
                .child("Agent")
                .child(div().text_color(theme::accent()).child("ory")),
        )
        .child(
            div()
                .mt(px(8.0))
                .size(px(26.0))
                .rounded_full()
                .border_3()
                .border_color(theme::accent())
                .with_animation(
                    "loader-ring",
                    Animation::new(Duration::from_millis(900)).repeat(),
                    |el, t| el.opacity(0.5 + 0.5 * (t * std::f32::consts::TAU).sin().abs()),
                ),
        )
        .child(
            div()
                .text_size(px(13.0))
                .text_color(theme::text_faint())
                .child(message.into()),
        )
}
