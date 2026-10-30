//! Sessions page: tab strip over the active session's terminal.
//!
//! Tabs are done; split panes are not (one terminal per tab).

use gpui::{
    Context, EventEmitter, InteractiveElement, IntoElement, ParentElement, Render,
    StatefulInteractiveElement, Styled, Window, div, prelude::*, px,
};

use crate::route::{GoBack, Navigate};
use crate::state::Stores;
use crate::state::sessions;
use crate::ui::components::button::{ButtonVariant, button};
use crate::ui::icons::{Icon, icon};
use crate::ui::shell::sidebar::{status_dot, status_label};
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
        cx.observe(&stores.prefs, |this, prefs, cx| {
            // Terminals follow the font size preference.
            let size = prefs.read(cx).prefs.terminal_font_size;
            let terms: Vec<_> = this
                .stores
                .sessions
                .read(cx)
                .items
                .iter()
                .map(|s| s.terminal.clone())
                .collect();
            for t in terms {
                t.update(cx, |t, cx| t.set_font_size(size, cx));
            }
        })
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

        let active_id = store.active.clone();
        let tabs: Vec<_> = store
            .items
            .iter()
            .map(|s| (s.id.clone(), s.name.clone(), s.status))
            .collect();
        let terminal = active.map(|s| s.terminal.clone());
        let status = active.map(|s| (s.status, s.cwd.display().to_string()));
        let selected_project = self.stores.projects.read(cx).selected.clone();
        let has_projects = !self.stores.projects.read(cx).items.is_empty();

        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .child(
                div()
                    .id("tab-strip")
                    .flex()
                    .flex_none()
                    .items_end()
                    .h(px(36.0))
                    .px(px(6.0))
                    .gap(px(2.0))
                    .bg(theme::bg_chrome())
                    .border_b_1()
                    .border_color(theme::border_subtle())
                    .overflow_x_scroll()
                    .children(tabs.into_iter().map(|(id, name, st)| {
                        let (pick, close) = (id.clone(), id.clone());
                        let is_active = active_id.as_deref() == Some(id.as_str());
                        div()
                            .flex()
                            .flex_none()
                            .items_center()
                            .gap(px(8.0))
                            .h(px(30.0))
                            .pl(px(10.0))
                            .pr(px(4.0))
                            .rounded_t(px(6.0))
                            .bg(if is_active {
                                theme::bg_terminal()
                            } else {
                                gpui::rgba(0x00000000)
                            })
                            .child(
                                div()
                                    .id(gpui::SharedString::from(format!("tab-{}", id.clone())))
                                    .flex()
                                    .items_center()
                                    .gap(px(8.0))
                                    .h_full()
                                    .cursor_pointer()
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.stores
                                            .sessions
                                            .update(cx, |s, cx| s.select(&pick, cx));
                                        // Re-focus even when the tab was already selected.
                                        this.focused = None;
                                        cx.notify();
                                    }))
                                    .child(status_dot(st))
                                    .child(
                                        div()
                                            .text_size(px(12.5))
                                            .text_color(if is_active {
                                                theme::text()
                                            } else {
                                                theme::text_muted()
                                            })
                                            .child(name),
                                    ),
                            )
                            .child(
                                div()
                                    .id(gpui::SharedString::from(format!("tab-close-{}", id)))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .size(px(20.0))
                                    .rounded(px(4.0))
                                    .cursor_pointer()
                                    .hover(|s| s.bg(theme::hover()))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.stores
                                            .sessions
                                            .update(cx, |s, cx| s.close(&close, cx));
                                    }))
                                    .child(
                                        icon(Icon::Close, px(12.0)).text_color(theme::text_muted()),
                                    ),
                            )
                    }))
                    .when_some(selected_project.clone(), |s, project_id| {
                        s.child(
                            div()
                                .id("tab-new")
                                .flex()
                                .flex_none()
                                .items_center()
                                .justify_center()
                                .size(px(30.0))
                                .rounded(px(6.0))
                                .cursor_pointer()
                                .hover(|s| s.bg(theme::hover()))
                                .on_click(cx.listener(move |_, _, _, cx| {
                                    sessions::begin_new(&project_id, cx);
                                }))
                                .child(icon(Icon::Plus, px(15.0)).text_color(theme::text())),
                        )
                    }),
            )
            .child(match terminal {
                Some(t) => div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .child(t)
                    .when_some(status, |s, (st, cwd)| {
                        s.child(
                            div()
                                .flex()
                                .flex_none()
                                .items_center()
                                .gap(px(8.0))
                                .h(px(24.0))
                                .px(px(10.0))
                                .bg(theme::bg_chrome())
                                .border_t_1()
                                .border_color(theme::border_subtle())
                                .text_size(px(11.0))
                                .text_color(theme::text_muted())
                                .child(status_dot(st))
                                .child(status_label(st))
                                .child(div().truncate().child(cwd)),
                        )
                    })
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
                            .child("No session open"),
                    )
                    .child(
                        div()
                            .text_size(px(13.0))
                            .text_color(theme::text_muted())
                            .child(if has_projects {
                                "Start Claude Code in the selected project."
                            } else {
                                "Add a project folder from the sidebar first."
                            }),
                    )
                    .when_some(selected_project, |s, project_id| {
                        s.child(button(
                            "empty-new-session",
                            "New session",
                            ButtonVariant::Classic,
                            cx.listener(move |_, _, _, cx| sessions::begin_new(&project_id, cx)),
                        ))
                    })
                    .into_any_element(),
            })
    }
}
