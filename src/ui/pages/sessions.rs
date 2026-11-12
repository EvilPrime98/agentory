//! Sessions page: the active session's terminal.
//!
//! Sessions are switched from the sidebar (or Ctrl+Tab); split panes are not done.

use std::collections::HashMap;

use gpui::{
    Context, Entity, EventEmitter, IntoElement, ParentElement, Render, Styled, Window, div,
    prelude::*, px,
};

use crate::i18n::t;
use crate::route::{GoBack, Navigate};
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
                if can_chat {
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
                                        stores
                                            .sessions
                                            .update(cx, |s, cx| s.set_view(&id, view, cx));
                                    },
                                )
                                .w(px(200.0)),
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
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .child(body)
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
