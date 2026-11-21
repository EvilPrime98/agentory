//! Sessions page: the active session's terminal.
//!
//! Sessions are switched from the sidebar (or Ctrl+Tab); split panes are not done.

use std::collections::HashMap;
use std::time::Duration;

use gpui::{
    Animation, AnimationExt as _, Context, Entity, EventEmitter, IntoElement, ParentElement,
    Render, Styled, Window, div, prelude::*, px,
};

use crate::i18n::t;
use crate::route::{GoBack, Navigate};
use crate::speech::controller::Phase;
use crate::state::Stores;
use crate::state::prefs::SessionView;
use crate::state::sessions;
use crate::ui::chat::ChatView;
use crate::ui::components::button::{ButtonVariant, button};
use crate::ui::components::segmented::segmented;
use crate::ui::icons::{Icon, icon};
use crate::ui::theme;

pub struct SessionsPage {
    stores: Stores,
    focused: Option<(String, SessionView)>,
    chats: HashMap<String, Entity<ChatView>>,
}

impl EventEmitter<Navigate> for SessionsPage {}
impl EventEmitter<GoBack> for SessionsPage {}

impl SessionsPage {
    pub fn new(stores: Stores, cx: &mut Context<Self>) -> Self {
        cx.observe(&stores.sessions, |_, _, cx| cx.notify())
            .detach();
        cx.observe(&stores.projects, |_, _, cx| cx.notify())
            .detach();
        cx.observe(&stores.speech, |_, _, cx| cx.notify()).detach();
        cx.observe(&stores.prefs, |_, _, cx| cx.notify()).detach();
        Self {
            stores,
            focused: None,
            chats: HashMap::new(),
        }
    }
}

impl Render for SessionsPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let live: Vec<String> = self
            .stores
            .sessions
            .read(cx)
            .items
            .iter()
            .map(|s| s.id.clone())
            .collect();
        self.chats.retain(|id, _| live.contains(id));

        let current = self.stores.sessions.read(cx).active_session().map(|s| {
            let chat = (s.view == SessionView::Chat && s.kind.is_agent())
                .then(|| s.claude_id.clone())
                .flatten();
            (
                s.id.clone(),
                s.name.clone(),
                s.kind.is_agent() && s.claude_id.is_some(),
                s.terminal.clone(),
                chat,
            )
        });
        let mut body = None;
        let mut header = None;
        let mut overlay = None;
        match current {
            Some((id, name, can_chat, terminal, chat_id)) => {
                let view = if chat_id.is_some() {
                    SessionView::Chat
                } else {
                    SessionView::Terminal
                };
                let focus = match chat_id {
                    Some(claude_id) => {
                        let stores = self.stores.clone();
                        let chat = self
                            .chats
                            .entry(id.clone())
                            .or_insert_with(|| {
                                cx.new(|cx| {
                                    ChatView::new(
                                        stores,
                                        id.clone(),
                                        claude_id,
                                        terminal.clone(),
                                        cx,
                                    )
                                })
                            })
                            .clone();
                        let handle = gpui::Focusable::focus_handle(chat.read(cx), cx);
                        body = Some(chat.into_any_element());
                        handle
                    }
                    None => {
                        body = Some(terminal.clone().into_any_element());
                        gpui::Focusable::focus_handle(terminal.read(cx), cx)
                    }
                };
                if self.focused.as_ref() != Some(&(id.clone(), view)) {
                    self.focused = Some((id.clone(), view));
                    window.focus(&focus, cx);
                }
                overlay = speech_overlay(&self.stores, &id, cx);
                let mic = self
                    .stores
                    .prefs
                    .read(cx)
                    .prefs
                    .speech_enabled
                    .then(|| mic_button(&self.stores, &id, cx));
                if can_chat || mic.is_some() {
                    let stores = self.stores.clone();
                    header = Some(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap(px(12.0))
                            .h(px(40.0))
                            .px(px(12.0))
                            .flex_none()
                            .border_b_1()
                            .border_color(theme::border_subtle())
                            .child(
                                div()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_size(px(13.0))
                                    .text_color(theme::text_secondary())
                                    .child(name),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(10.0))
                                    .children(mic)
                                    .when(can_chat, |d| {
                                        d.child(
                                            segmented(
                                                "session-view",
                                                SessionView::ALL
                                                    .iter()
                                                    .map(|v| t(v.label_key()).into())
                                                    .collect(),
                                                SessionView::ALL
                                                    .iter()
                                                    .position(|v| *v == view)
                                                    .unwrap_or(0),
                                                move |i, _, cx| {
                                                    let view = SessionView::ALL[i];
                                                    let id = id.clone();
                                                    stores.sessions.update(cx, |s, cx| {
                                                        s.set_view(&id, view, cx)
                                                    });
                                                },
                                            )
                                            .w(px(200.0)),
                                        )
                                    }),
                            ),
                    );
                }
            }
            None => self.focused = None,
        }
        let selected_project = self.stores.projects.read(cx).selected.clone();
        let has_projects = !self.stores.projects.read(cx).items.is_empty();

        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .children(header)
            .child(match body {
                Some(body) => div()
                    .relative()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .child(body)
                    .children(overlay)
                    .into_any_element(),
                None => div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .items_center()
                    .justify_center()
                    .gap(px(10.0))
                    .child(icon(Icon::Terminal, px(40.0)).text_color(theme::text_muted()))
                    .child(
                        div()
                            .text_size(px(15.0))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child(t("sessions.empty.title")),
                    )
                    .child(
                        div()
                            .text_size(px(13.0))
                            .text_color(theme::text_muted())
                            .child(if has_projects {
                                t("sessions.empty.start")
                            } else {
                                t("sessions.empty.add_project")
                            }),
                    )
                    .when_some(selected_project, |s, project_id| {
                        s.child(button(
                            "empty-new-session",
                            t("sessions.new"),
                            ButtonVariant::Classic,
                            cx.listener(move |_, _, _, cx| sessions::begin_new(&project_id, cx)),
                        ))
                    })
                    .into_any_element(),
            })
    }
}

fn speech_overlay(
    stores: &Stores,
    session_id: &str,
    cx: &Context<SessionsPage>,
) -> Option<gpui::AnyElement> {
    let speech = stores.speech.read(cx);
    if speech.session() != Some(session_id) {
        return None;
    }
    let phase = speech.phase();
    let (color, period, inward, size, alpha) = match phase {
        Phase::Idle | Phase::Sending => return None,
        Phase::Loading => (theme::text_secondary(), 1600, false, 90.0, 0.18),
        Phase::Listening => (theme::accent(), 2600, false, 100.0, 0.18),
        Phase::Speaking => (theme::accent(), 1100, false, 140.0, 0.35),
        Phase::Transcribing => (theme::text_secondary(), 1300, true, 120.0, 0.3),
    };
    let core = size * 0.2;
    let ripple = move |n: usize| {
        div()
            .absolute()
            .rounded_full()
            .border_2()
            .border_color(color)
            .with_animation(
                ("speech-ripple", phase as usize * 4 + n),
                Animation::new(Duration::from_millis(period)).repeat(),
                move |el, t| {
                    let t = (t + n as f32 * 0.5) % 1.0;
                    let t = if inward { 1.0 - t } else { t };
                    el.size(px(core + (size - core) * t))
                        .opacity(alpha * (1.0 - t))
                },
            )
    };
    Some(
        div()
            .absolute()
            .inset_0()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(14.0))
            .child(
                div()
                    .relative()
                    .size(px(size))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(ripple(0))
                    .child(ripple(1))
                    .child(
                        div()
                            .size(px(core))
                            .rounded_full()
                            .bg(color)
                            .with_animation(
                                ("speech-core", phase as usize),
                                Animation::new(Duration::from_millis(period)).repeat(),
                                move |el, t| {
                                    el.opacity(
                                        alpha
                                            * (0.6 + 0.4 * (t * std::f32::consts::TAU).sin().abs()),
                                    )
                                },
                            ),
                    ),
            )
            .into_any_element(),
    )
}

fn mic_button(stores: &Stores, session_id: &str, cx: &Context<SessionsPage>) -> gpui::AnyElement {
    let speech = stores.speech.read(cx);
    let here = speech.session() == Some(session_id);
    let phase = if here { speech.phase() } else { Phase::Idle };
    let (label, color) = if let Some(bytes) = speech.download {
        let total = crate::speech::model::total_size().max(1);
        (
            t_args_pct("speech.downloading", bytes * 100 / total),
            theme::text_secondary(),
        )
    } else if !speech.installed {
        (t("speech.install"), theme::text_secondary())
    } else {
        match phase {
            Phase::Idle if speech.error.is_some() => (t("speech.error"), theme::error()),
            Phase::Idle => (t("speech.idle"), theme::text_secondary()),
            Phase::Loading => (t("speech.loading"), theme::text_secondary()),
            Phase::Listening => (t("speech.listening"), theme::accent()),
            Phase::Speaking => (t("speech.speaking"), theme::accent()),
            Phase::Transcribing => (t("speech.transcribing"), theme::text_secondary()),
            Phase::Sending => (t("speech.sending"), theme::accent()),
        }
    };
    let active = !matches!(phase, Phase::Idle);
    let installed = speech.installed;
    let downloading = speech.download.is_some();
    let stores = stores.clone();
    let id = session_id.to_string();
    div()
        .id("session-mic")
        .flex()
        .items_center()
        .gap(px(6.0))
        .h(px(26.0))
        .px(px(10.0))
        .rounded(px(4.0))
        .border_1()
        .border_color(if active {
            theme::accent()
        } else {
            theme::border_subtle()
        })
        .text_size(px(12.0))
        .text_color(color)
        .cursor_pointer()
        .hover(|s| s.bg(theme::hover()))
        .on_click(move |_, _, cx| {
            if downloading {
                return;
            }
            stores.speech.update(cx, |s, cx| {
                if installed {
                    s.toggle(&id, cx);
                } else {
                    s.download_model(cx);
                }
            });
        })
        .child(icon(Icon::Mic, px(14.0)).text_color(color))
        .child(label)
        .into_any_element()
}

fn t_args_pct(key: &str, pct: u64) -> String {
    crate::i18n::t_args(key, &[("percent", &pct.to_string())])
}
