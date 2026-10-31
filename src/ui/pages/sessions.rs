//! Sessions page: the active session's terminal.
//!
//! Sessions are switched from the sidebar (or Ctrl+Tab); split panes are not done.

use gpui::{
    Context, EventEmitter, IntoElement, ParentElement, Render, Styled, Window, div, prelude::*, px,
};

use crate::route::{GoBack, Navigate};
use crate::state::Stores;
use crate::state::sessions;
use crate::i18n::t;
use crate::ui::components::button::{ButtonVariant, button};
use crate::ui::icons::{Icon, icon};
use crate::ui::theme;

pub struct SessionsPage {
    stores: Stores,
    /// Session whose terminal was last given keyboard focus.
    focused: Option<String>,
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
        }
    }
}

impl Render for SessionsPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // The newly selected session's terminal takes keyboard focus once per selection.
        let current = self
            .stores
            .sessions
            .read(cx)
            .active_session()
            .map(|s| (s.id.clone(), s.terminal.clone()));
        match current {
            Some((id, terminal)) if self.focused.as_deref() != Some(id.as_str()) => {
                self.focused = Some(id);
                let handle = gpui::Focusable::focus_handle(terminal.read(cx), cx);
                window.focus(&handle, cx);
            }
            Some(_) => {}
            None => self.focused = None,
        }
        let store = self.stores.sessions.read(cx);
        let active = store.active_session();

        let terminal = active.map(|s| s.terminal.clone());
        let selected_project = self.stores.projects.read(cx).selected.clone();
        let has_projects = !self.stores.projects.read(cx).items.is_empty();

        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .child(match terminal {
                Some(t) => div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .child(t)
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
