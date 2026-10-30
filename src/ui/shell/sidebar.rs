//! Sidebar: projects with their sessions nested underneath, each with a status dot.

use std::collections::HashSet;

use gpui::{
    Context, EventEmitter, InteractiveElement, IntoElement, ParentElement, Render, Rgba,
    StatefulInteractiveElement, Styled, Window, div, prelude::*, px,
};

use crate::model::{Project, SessionStatus};
use crate::route::{Navigate, Route};
use crate::runtime;
use crate::state::Stores;
use crate::state::sessions;
use crate::ui::confirm::{self, ConfirmOptions};
use crate::ui::icons::{Icon, icon};
use crate::ui::theme;

const REPO_URL: &str = "https://github.com/EvilPrime98/agentory";

pub struct Sidebar {
    stores: Stores,
    /// Projects whose session list is folded away.
    collapsed: HashSet<String>,
    picker_open: bool,
}

impl EventEmitter<Navigate> for Sidebar {}

pub fn status_color(status: SessionStatus) -> Rgba {
    match status {
        SessionStatus::Starting => theme::text_muted(),
        SessionStatus::Working => theme::accent(),
        SessionStatus::NeedsInput => theme::warning(),
        SessionStatus::Idle => theme::success(),
        SessionStatus::Exited => gpui::rgb(0x55555c),
    }
}

pub fn status_label(status: SessionStatus) -> &'static str {
    match status {
        SessionStatus::Starting => "Starting",
        SessionStatus::Working => "Working",
        SessionStatus::NeedsInput => "Needs input",
        SessionStatus::Idle => "Idle",
        SessionStatus::Exited => "Exited",
    }
}

pub fn status_dot(status: SessionStatus) -> impl IntoElement {
    div()
        .flex_none()
        .size(px(8.0))
        .rounded_full()
        .bg(status_color(status))
}

impl Sidebar {
    pub fn new(stores: Stores, cx: &mut Context<Self>) -> Self {
        cx.observe(&stores.projects, |_, _, cx| cx.notify())
            .detach();
        cx.observe(&stores.sessions, |_, _, cx| cx.notify())
            .detach();
        Self {
            stores,
            collapsed: HashSet::new(),
            picker_open: false,
        }
    }

    fn add_project(&mut self, cx: &mut Context<Self>) {
        if self.picker_open {
            return;
        }
        self.picker_open = true;
        cx.spawn(async move |this, cx| {
            let picked = runtime::run(async {
                rfd::AsyncFileDialog::new()
                    .set_title("Add a project folder")
                    .pick_folder()
                    .await
                    .map(|h| h.path().to_path_buf())
            })
            .await;
            this.update(cx, |this, cx| {
                this.picker_open = false;
                if let Some(path) = picked {
                    this.stores.projects.update(cx, |p, cx| {
                        p.add(path, cx);
                    });
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn remove_project(&mut self, id: String, cx: &mut Context<Self>) {
        let stores = self.stores.clone();
        let running = stores
            .sessions
            .read(cx)
            .for_project(&id)
            .filter(|s| s.status != SessionStatus::Exited)
            .count();
        let remove = move |cx: &mut gpui::App| {
            stores.sessions.update(cx, |s, cx| s.close_project(&id, cx));
            stores.projects.update(cx, |p, cx| p.remove(&id, cx));
        };
        if running == 0 {
            // Entity updates need an `App`; `Context` derefs to it.
            remove(cx);
            return;
        }
        let options = ConfirmOptions::new(
            "Remove project",
            format!("{running} running session(s) of this project will be terminated."),
        )
        .labels("Remove", "Cancel");
        confirm::ask(cx, options, move |answer, _, cx| {
            if answer == Some(true) {
                remove(cx);
            }
        });
    }

    fn project_block(&self, project: &Project, cx: &mut Context<Self>) -> gpui::AnyElement {
        let projects = self.stores.projects.read(cx);
        let selected = projects.selected.as_deref() == Some(project.id.as_str());
        let collapsed = self.collapsed.contains(&project.id);
        let sessions: Vec<_> = self
            .stores
            .sessions
            .read(cx)
            .for_project(&project.id)
            .map(|s| (s.id.clone(), s.name.clone(), s.status, s.worktree.is_some()))
            .collect();
        let active = self.stores.sessions.read(cx).active.clone();

        let (toggle_id, select_id, new_id, remove_id) = (
            project.id.clone(),
            project.id.clone(),
            project.id.clone(),
            project.id.clone(),
        );

        div()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex()
                    .items_center()
                    .h(px(32.0))
                    .px(px(6.0))
                    .rounded(px(6.0))
                    .when(selected, |s| s.bg(theme::hover()))
                    .child(
                        div()
                            .id(gpui::SharedString::from(format!(
                                "chevron-{}",
                                project.id.clone()
                            )))
                            .flex_none()
                            .size(px(20.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if !this.collapsed.remove(&toggle_id) {
                                    this.collapsed.insert(toggle_id.clone());
                                }
                                cx.notify();
                            }))
                            .child(
                                icon(
                                    if collapsed {
                                        Icon::ChevronRight
                                    } else {
                                        Icon::ChevronDown
                                    },
                                    px(14.0),
                                )
                                .text_color(theme::text_muted()),
                            ),
                    )
                    .child(
                        div()
                            .id(gpui::SharedString::from(format!(
                                "project-{}",
                                project.id.clone()
                            )))
                            .flex()
                            .flex_1()
                            .min_w_0()
                            .items_center()
                            .gap(px(8.0))
                            .h_full()
                            .cursor_pointer()
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.stores
                                    .projects
                                    .update(cx, |p, cx| p.select(&select_id, cx));
                                cx.emit(Navigate(Route::Sessions));
                                let _ = window;
                            }))
                            .child(icon(Icon::Folder, px(16.0)).text_color(theme::text_muted()))
                            .child(
                                div()
                                    .truncate()
                                    .text_size(px(13.0))
                                    .text_color(theme::text())
                                    .child(project.name.clone()),
                            ),
                    )
                    .child(
                        div()
                            .id(gpui::SharedString::from(format!(
                                "new-session-{}",
                                project.id.clone()
                            )))
                            .flex_none()
                            .size(px(24.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(4.0))
                            .cursor_pointer()
                            .hover(|s| s.bg(theme::hover()))
                            .on_click(cx.listener(move |this, event: &gpui::ClickEvent, _, cx| {
                                this.stores
                                    .projects
                                    .update(cx, |p, cx| p.select(&new_id, cx));
                                cx.emit(Navigate(Route::Sessions));
                                // Alt-click: this session only, the other way round from the
                                // project's remembered choice.
                                let remembered = this
                                    .stores
                                    .projects
                                    .read(cx)
                                    .get(&new_id)
                                    .and_then(|p| p.use_worktree);
                                match remembered {
                                    Some(worktree) if event.modifiers().alt => {
                                        sessions::begin_new_with(&new_id, !worktree, cx)
                                    }
                                    _ => sessions::begin_new(&new_id, cx),
                                }
                            }))
                            .child(icon(Icon::Plus, px(15.0)).text_color(theme::text())),
                    )
                    .child(
                        div()
                            .id(gpui::SharedString::from(format!(
                                "remove-project-{}",
                                project.id.clone()
                            )))
                            .flex_none()
                            .size(px(24.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(4.0))
                            .cursor_pointer()
                            .hover(|s| s.bg(theme::hover()))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.remove_project(remove_id.clone(), cx)
                            }))
                            .child(icon(Icon::Trash, px(14.0)).text_color(theme::text_muted())),
                    ),
            )
            .when(!collapsed, |s| {
                s.children(sessions.into_iter().map(|(id, name, status, worktree)| {
                    let (pick_id, close_id) = (id.clone(), id.clone());
                    let is_active = active.as_deref() == Some(id.as_str());
                    div()
                        .flex()
                        .items_center()
                        .h(px(28.0))
                        .ml(px(26.0))
                        .px(px(6.0))
                        .rounded(px(6.0))
                        .when(is_active, |s| s.bg(theme::accent_soft()))
                        .child(
                            div()
                                .id(gpui::SharedString::from(format!("session-{}", id.clone())))
                                .flex()
                                .flex_1()
                                .min_w_0()
                                .items_center()
                                .gap(px(8.0))
                                .h_full()
                                .cursor_pointer()
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.stores
                                        .sessions
                                        .update(cx, |s, cx| s.select(&pick_id, cx));
                                    cx.emit(Navigate(Route::Sessions));
                                }))
                                .child(status_dot(status))
                                .child(
                                    div()
                                        .truncate()
                                        .text_size(px(12.5))
                                        .text_color(if status == SessionStatus::Exited {
                                            theme::text_muted()
                                        } else {
                                            theme::text()
                                        })
                                        .child(name),
                                )
                                .when(worktree, |s| {
                                    s.child(
                                        icon(Icon::Branch, px(12.0))
                                            .text_color(theme::text_muted()),
                                    )
                                }),
                        )
                        .child(
                            div()
                                .id(gpui::SharedString::from(format!(
                                    "close-session-{}",
                                    id.clone()
                                )))
                                .flex_none()
                                .size(px(20.0))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded(px(4.0))
                                .cursor_pointer()
                                .hover(|s| s.bg(theme::hover()))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.stores
                                        .sessions
                                        .update(cx, |s, cx| s.close(&close_id, cx));
                                }))
                                .child(icon(Icon::Close, px(12.0)).text_color(theme::text_muted())),
                        )
                }))
            })
            .into_any_element()
    }
}

impl Render for Sidebar {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let projects: Vec<Project> = self.stores.projects.read(cx).items.clone();
        let blocks: Vec<_> = projects.iter().map(|p| self.project_block(p, cx)).collect();
        let empty = projects.is_empty();

        div()
            .flex()
            .flex_col()
            .flex_none()
            .w(theme::sidebar_width())
            .h_full()
            .bg(theme::bg_panel())
            .border_r_1()
            .border_color(theme::border_subtle())
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_between()
                    .h(px(40.0))
                    .px(px(12.0))
                    .child(
                        div()
                            .text_size(px(11.0))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(theme::text_muted())
                            .child("PROJECTS"),
                    )
                    .child(
                        div()
                            .id("add-project")
                            .flex()
                            .items_center()
                            .justify_center()
                            .size(px(26.0))
                            .rounded(px(6.0))
                            .cursor_pointer()
                            .hover(|s| s.bg(theme::hover()))
                            .on_click(cx.listener(|this, _, _, cx| this.add_project(cx)))
                            .child(icon(Icon::FolderPlus, px(16.0)).text_color(theme::text())),
                    ),
            )
            .child(
                div()
                    .id("projects-scroll")
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .gap(px(2.0))
                    .px(px(6.0))
                    .pb(px(8.0))
                    .overflow_y_scroll()
                    .children(blocks)
                    .when(empty, |s| {
                        s.child(
                            div()
                                .px(px(8.0))
                                .py(px(12.0))
                                .text_size(px(12.5))
                                .text_color(theme::text_muted())
                                .child("Add a project folder to start a Claude Code session."),
                        )
                    }),
            )
            .child(
                div()
                    .id("sidebar-footer")
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_center()
                    .gap(px(4.0))
                    .h(px(32.0))
                    .border_t_1()
                    .border_color(theme::border_subtle())
                    .text_size(px(11.0))
                    .text_color(theme::text_muted())
                    .cursor_pointer()
                    .hover(|s| s.text_color(theme::text()))
                    .on_click(|_, _, cx| cx.open_url(REPO_URL))
                    .child("Made with")
                    .child(icon(Icon::Heart, px(11.0)).text_color(gpui::rgb(0xe5484d)))
                    .child("by EvilPrime98"),
            )
    }
}
