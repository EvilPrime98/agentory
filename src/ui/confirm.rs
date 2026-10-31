//! Confirm dialog. The result is delivered through a callback:
//! `Some(true)` confirmed, `Some(false)` cancelled, `None` dismissed (Esc / backdrop).

use gpui::{
    App, AppContext as _, Context, Entity, FocusHandle, Focusable, Global, InteractiveElement,
    IntoElement, MouseButton, ParentElement, Render, SharedString, StatefulInteractiveElement,
    Styled, Window, actions, div, px,
};

use crate::i18n::t;
use crate::ui::components::button::{ButtonVariant, button};
use crate::ui::theme;

actions!(confirm, [Dismiss]);

pub struct ConfirmOptions {
    pub title: SharedString,
    pub message: SharedString,
    pub confirm_label: SharedString,
    pub cancel_label: SharedString,
    /// Shows a checkbox (see `checkbox`); its state is passed to the callback.
    pub dont_ask_again: bool,
    pub checkbox_label: SharedString,
    pub checkbox_checked: bool,
}

impl ConfirmOptions {
    pub fn new(title: impl Into<SharedString>, message: impl Into<SharedString>) -> Self {
        Self {
            title: title.into(),
            message: message.into(),
            confirm_label: t("common.confirm").into(),
            cancel_label: t("common.cancel").into(),
            dont_ask_again: false,
            checkbox_label: t("confirm.dont_ask_again").into(),
            checkbox_checked: false,
        }
    }

    /// Show a checkbox with a custom label and initial state.
    pub fn checkbox(mut self, label: impl Into<SharedString>, checked: bool) -> Self {
        self.dont_ask_again = true;
        self.checkbox_label = label.into();
        self.checkbox_checked = checked;
        self
    }

    pub fn labels(mut self, confirm: impl Into<SharedString>, cancel: impl Into<SharedString>) -> Self {
        self.confirm_label = confirm.into();
        self.cancel_label = cancel.into();
        self
    }
}

type Callback = Box<dyn FnOnce(Option<bool>, bool, &mut App)>;

struct Pending {
    options: ConfirmOptions,
    dont_ask: bool,
    on_answer: Callback,
}

pub struct ConfirmHost {
    pending: Option<Pending>,
    focus: FocusHandle,
}

struct ConfirmGlobal(Entity<ConfirmHost>);
impl Global for ConfirmGlobal {}

pub fn init(cx: &mut App) -> Entity<ConfirmHost> {
    cx.bind_keys([gpui::KeyBinding::new(
        "escape",
        Dismiss,
        Some("ConfirmModal"),
    )]);
    let host = cx.new(|cx| ConfirmHost {
        pending: None,
        focus: cx.focus_handle(),
    });
    cx.set_global(ConfirmGlobal(host.clone()));
    host
}

/// Ask the user. `on_answer(answer, dont_ask_again, cx)` runs once. A second `ask` while one is
/// open dismisses the first (answer `None`).
pub fn ask(
    cx: &mut App,
    options: ConfirmOptions,
    on_answer: impl FnOnce(Option<bool>, bool, &mut App) + 'static,
) {
    let Some(host) = cx.try_global::<ConfirmGlobal>().map(|g| g.0.clone()) else {
        return;
    };
    let previous = host.update(cx, |host, cx| {
        let previous = host.pending.take();
        host.pending = Some(Pending {
            dont_ask: options.checkbox_checked,
            options,
            on_answer: Box::new(on_answer),
        });
        cx.notify();
        previous
    });
    if let Some(p) = previous {
        (p.on_answer)(None, false, cx);
    }
}

impl ConfirmHost {
    fn resolve(&mut self, answer: Option<bool>, cx: &mut Context<Self>) {
        if let Some(p) = self.pending.take() {
            let dont_ask = p.dont_ask;
            cx.notify();
            // Run after this update so the callback may call `ask` again.
            cx.defer(move |cx| (p.on_answer)(answer, dont_ask, cx));
        }
    }
}

impl Focusable for ConfirmHost {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for ConfirmHost {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(p) = &self.pending else {
            return div().into_any_element();
        };
        if !self.focus.is_focused(window) {
            window.focus(&self.focus, cx);
        }
        let dont_ask = p.dont_ask;
        let checkbox = p.options.dont_ask_again.then(|| {
            let mut tick = div()
                .size(px(16.0))
                .rounded(px(4.0))
                .border_1()
                .border_color(theme::accent());
            if dont_ask {
                tick = tick.bg(theme::accent());
            }
            div()
                .id("confirm-dont-ask")
                .flex()
                .items_center()
                .gap(px(8.0))
                .cursor_pointer()
                .text_size(px(13.0))
                .text_color(theme::text_muted())
                .on_click(cx.listener(|this, _, _, cx| {
                    if let Some(p) = &mut this.pending {
                        p.dont_ask = !p.dont_ask;
                        cx.notify();
                    }
                }))
                .child(tick)
                .child(p.options.checkbox_label.clone())
        });

        div()
            .id("confirm-overlay")
            .key_context("ConfirmModal")
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &Dismiss, _, cx| this.resolve(None, cx)))
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .p(px(16.0))
            .bg(gpui::rgba(0x0000008c))
            .occlude()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| this.resolve(None, cx)),
            )
            .child(
                div()
                    .id("confirm-modal")
                    // Clicks inside must not reach the dismissing backdrop.
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .flex()
                    .flex_col()
                    .gap(px(16.0))
                    .w(px(360.0))
                    .max_w_full()
                    .p(px(20.0))
                    .bg(theme::bg_modal())
                    .border_1()
                    .border_color(theme::border_subtle())
                    .rounded(px(14.0))
                    .child(
                        div()
                            .text_size(px(15.0))
                            .font_weight(gpui::FontWeight::BOLD)
                            .text_color(theme::text())
                            .child(p.options.title.clone()),
                    )
                    .child(
                        div()
                            .text_size(px(13.0))
                            .line_height(px(19.5))
                            .text_color(gpui::rgb(0xb8b8b8))
                            .child(p.options.message.clone()),
                    )
                    .children(checkbox)
                    .child(
                        div()
                            .flex()
                            .justify_end()
                            .gap(px(8.0))
                            .mt(px(4.0))
                            .child(button(
                                "confirm-cancel",
                                p.options.cancel_label.clone(),
                                ButtonVariant::Secondary,
                                cx.listener(|this, _, _, cx| this.resolve(Some(false), cx)),
                            ))
                            .child(button(
                                "confirm-ok",
                                p.options.confirm_label.clone(),
                                ButtonVariant::Classic,
                                cx.listener(|this, _, _, cx| this.resolve(Some(true), cx)),
                            )),
                    ),
            )
            .into_any_element()
    }
}
