//! Sidebar: projects with their sessions nested underneath, each with a status dot.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use gpui::{
    Animation, AnimationExt, AnyElement, Context, EventEmitter, InteractiveElement, IntoElement,
    ParentElement, Render, Rgba, StatefulInteractiveElement, Styled, Transformation, Window,
    deferred, div, prelude::*, px, radians,
};

use crate::backend::git;
use crate::backend::spm::store;
use crate::i18n::{t, t_args, t_n};
use crate::model::{Project, SessionStatus};
use crate::platform::shells::{self, Shell};
use crate::route::{Navigate, Route};
use crate::runtime;
use crate::state::Stores;
use crate::state::sessions::{self, SessionKind, ShellDir};
use crate::ui::command_editor;
use crate::ui::components::checkbox::checkbox;
use crate::ui::components::context_menu::{MenuItem, context_menu};
use crate::ui::confirm::{self, ConfirmOptions};
use crate::ui::icons::{Icon, icon};
use crate::ui::theme;

const REPO_URL: &str = "https://github.com/EvilPrime98/agentory";
/// A click on the menu button right after the menu closed itself (mouse-down-out) is the same
/// click: it must not reopen it.
const REOPEN_GUARD_MS: u128 = 200;
const FOLD_DURATION: Duration = Duration::from_millis(180);
/// Height of every session and terminal row.
const ROW_H: f32 = 28.0;
/// Height of the AGENTS / TERMINALS label above their rows.
const SECTION_HEADER_H: f32 = 24.0;

/// The small caps label above a group of rows in a project's list.
fn section_header(label: String) -> gpui::Div {
    div()
        .h(px(SECTION_HEADER_H))
        .flex()
        .items_end()
        .ml(px(26.0))
        .px(px(6.0))
        .pb(px(2.0))
        .text_size(px(10.5))
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .text_color(theme::text_muted())
        .child(label)
}

fn ease_out_cubic(t: f32) -> f32 {
    1.0 - (1.0 - t).powi(3)
}

/// The fold of one project's list toward `to` (0 = folded, 1 = open) from `from`.
#[derive(Clone, Copy)]
struct Fold {
    id: usize,
    from: f32,
    to: f32,
    started: Instant,
    duration: Duration,
}

impl Fold {
    /// Reverses `prev` from wherever it currently is, so a mid-animation click does not jump.
    fn toggle(prev: Option<Self>) -> Self {
        let (from, open) = prev.map_or((0.0, false), |f| (f.progress(), f.open()));
        let to = if open { 0.0 } else { 1.0 };
        Self {
            id: prev.map_or(0, |f| f.id + 1),
            from,
            to,
            started: Instant::now(),
            duration: FOLD_DURATION
                .mul_f32((to - from).abs())
                .max(Duration::from_millis(1)),
        }
    }

    fn open(&self) -> bool {
        self.to > 0.5
    }

    fn running(&self) -> bool {
        self.started.elapsed() < self.duration
    }

    fn progress(&self) -> f32 {
        let t = (self.started.elapsed().as_secs_f32() / self.duration.as_secs_f32()).min(1.0);
        self.from + (self.to - self.from) * ease_out_cubic(t)
    }
}

fn fold_chevron(size: f32, key: String, fold: Option<Fold>) -> AnyElement {
    let chevron = move |progress: f32| {
        icon(Icon::ChevronRight, px(size))
            .text_color(theme::text_muted())
            .with_transformation(Transformation::rotate(radians(
                progress * std::f32::consts::FRAC_PI_2,
            )))
    };
    match fold.filter(|f| f.running()) {
        Some(f) => chevron(f.from)
            .with_animation(
                (gpui::SharedString::from(key), f.id),
                Animation::new(f.duration).with_easing(ease_out_cubic),
                move |_, t| chevron(f.from + (f.to - f.from) * t),
            )
            .into_any_element(),
        None => chevron(if fold.is_some_and(|f| f.open()) {
            1.0
        } else {
            0.0
        })
        .into_any_element(),
    }
}

struct DraggedProject {
    id: String,
    name: String,
}

struct DragGhost(String);

impl Render for DragGhost {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .items_center()
            .gap(px(8.0))
            .h(px(32.0))
            .px(px(10.0))
            .rounded(px(6.0))
            .border_1()
            .border_color(theme::border_subtle())
            .bg(theme::bg_modal())
            .opacity(0.9)
            .child(icon(Icon::Folder, px(16.0)).text_color(theme::text_muted()))
            .child(
                div()
                    .text_size(px(13.0))
                    .text_color(theme::text())
                    .child(self.0.clone()),
            )
    }
}

const PAGES_MENU_W: f32 = 200.0;

const PAGES: [(&str, &str); 8] = [
    ("skills", "sidebar.skills"),
    ("docs", "sidebar.docs"),
    ("git", "sidebar.git"),
    ("github", "sidebar.github"),
    ("history", "sidebar.history"),
    ("workflows", "sidebar.workflows"),
    ("graphify", "sidebar.graphify"),
    ("commands", "sidebar.commands_extra"),
];

pub struct Sidebar {
    stores: Stores,
    drop_hint: Option<(String, bool)>,
    folds: HashMap<String, Fold>,
    picker_open: bool,
    shell_menu: Option<ShellMenu>,
    project_menu: Option<ProjectMenu>,
    isolated: std::cell::RefCell<HashMap<String, bool>>,
    menu_focus: gpui::FocusHandle,
    commands_folds: HashMap<String, Fold>,
    shell_menu_closed_at: Option<Instant>,
    pages_menu: bool,
    pages_menu_closed_at: Option<Instant>,
    compact: bool,
    /// The current route: only the Sessions page highlights a session, only Skills its row.
    route: Route,
}

struct ProjectMenu {
    project_id: String,
    position: gpui::Point<gpui::Pixels>,
    isolated: bool,
}

fn settings_file(project: &Path) -> PathBuf {
    project.join(".claude").join("settings.json")
}

/// The open "new terminal" menu of one project.
struct ShellMenu {
    project_id: String,
    shells: Vec<Shell>,
}

impl EventEmitter<Navigate> for Sidebar {}

pub fn status_color(status: SessionStatus) -> Rgba {
    match status {
        SessionStatus::Starting => theme::text_muted(),
        SessionStatus::Working => theme::accent(),
        SessionStatus::NeedsInput => theme::warning(),
        SessionStatus::Idle => theme::success(),
        SessionStatus::Exited => theme::status_exited(),
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
        cx.observe(&stores.selfbuild, |_, _, cx| cx.notify())
            .detach();
        cx.observe(&stores.git, |_, _, cx| cx.notify()).detach();
        cx.observe(&stores.prefs, |_, _, cx| cx.notify()).detach();
        Self::poll_git_count(&stores, cx);
        Self {
            stores,
            drop_hint: None,
            folds: HashMap::new(),
            picker_open: false,
            shell_menu: None,
            project_menu: None,
            isolated: Default::default(),
            menu_focus: cx.focus_handle(),
            commands_folds: HashMap::new(),
            shell_menu_closed_at: None,
            pages_menu: false,
            pages_menu_closed_at: None,
            compact: false,
            route: Route::home(),
        }
    }

    /// Keep the badge of the selected project fresh while the Git page is closed.
    fn poll_git_count(stores: &Stores, cx: &mut Context<Self>) {
        let stores = stores.clone();
        cx.spawn(async move |this, cx| {
            loop {
                let target = this.update(cx, |_, cx| {
                    let p = stores.projects.read(cx);
                    p.selected_project().map(|p| (p.id.clone(), p.path.clone()))
                });
                let Ok(target) = target else { break };
                if let Some((id, path)) = target {
                    let count = crate::runtime::run(async move {
                        let root = git::repo_root(&path).await.ok().flatten()?;
                        git::status(&root).await.ok().map(|s| s.total())
                    })
                    .await;
                    let git_store = stores.git.clone();
                    let updated = this.update(cx, |_, cx| {
                        git_store.update(cx, |g, cx| g.set_count(&id, count.unwrap_or(0), cx))
                    });
                    if updated.is_err() {
                        break;
                    }
                }
                cx.background_executor().timer(Duration::from_secs(5)).await;
            }
        })
        .detach();
    }

    pub fn set_route(&mut self, route: Route, cx: &mut Context<Self>) {
        if self.route != route {
            self.route = route;
            cx.notify();
        }
    }

    fn toggle_fold(&mut self, project_id: &str, cx: &mut Context<Self>) {
        let fold = Fold::toggle(self.folds.get(project_id).copied());
        self.folds.insert(project_id.to_string(), fold);
        Self::rerender_after(fold, cx);
    }

    fn toggle_commands(&mut self, project_id: &str, cx: &mut Context<Self>) {
        let fold = Fold::toggle(self.commands_folds.get(project_id).copied());
        self.commands_folds.insert(project_id.to_string(), fold);
        Self::rerender_after(fold, cx);
    }

    fn commands_open(&self, project_id: &str) -> bool {
        self.commands_folds
            .get(project_id)
            .is_some_and(|f| f.open())
    }

    fn rerender_after(fold: Fold, cx: &mut Context<Self>) {
        let wait = fold.duration + Duration::from_millis(16);
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(wait).await;
            this.update(cx, |_, cx| cx.notify()).ok();
        })
        .detach();
        cx.notify();
    }

    fn toggle_shell_menu(&mut self, project_id: &str, cx: &mut Context<Self>) {
        if self.shell_menu.is_some() {
            return self.close_shell_menu(cx);
        }
        if self
            .shell_menu_closed_at
            .is_none_or(|t| t.elapsed().as_millis() > REOPEN_GUARD_MS)
        {
            self.shell_menu = Some(ShellMenu {
                project_id: project_id.to_string(),
                shells: shells::detect(),
            });
            cx.notify();
        }
    }

    /// Select the project and show the sessions page, where a new terminal appears.
    fn show_project(&mut self, project_id: &str, cx: &mut Context<Self>) {
        self.stores
            .projects
            .update(cx, |p, cx| p.select(project_id, cx));
        cx.emit(Navigate(Route::Sessions));
    }

    fn open_project_menu(
        &mut self,
        project: &Project,
        position: gpui::Point<gpui::Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let isolated = self.is_isolated(project);
        self.project_menu = Some(ProjectMenu {
            project_id: project.id.clone(),
            position,
            isolated,
        });
        window.focus(&self.menu_focus, cx);
        cx.notify();
    }

    fn is_isolated(&self, project: &Project) -> bool {
        *self
            .isolated
            .borrow_mut()
            .entry(project.id.clone())
            .or_insert_with(|| {
                store::read_isolation(&settings_file(&project.path), &project.path)
            })
    }

    fn close_project_menu(&mut self, cx: &mut Context<Self>) {
        if self.project_menu.take().is_some() {
            cx.notify();
        }
    }

    fn toggle_isolate(&mut self, project_id: &str, cx: &mut Context<Self>) {
        let Some(path) = self
            .stores
            .projects
            .read(cx)
            .get(project_id)
            .map(|p| p.path.clone())
        else {
            return;
        };
        let file = settings_file(&path);
        let on = !store::read_isolation(&file, &path);
        let result = if on {
            store::apply_isolation(&file, &path)
        } else {
            store::remove_isolation(&file, &path)
        };
        match result {
            Ok(()) => {
                self.isolated.borrow_mut().insert(project_id.to_string(), on);
                cx.notify();
            }
            Err(e) => eprintln!("isolate: {e}"),
        }
    }

    fn project_menu_popup(&self, menu: &ProjectMenu, cx: &mut Context<Self>) -> AnyElement {
        let id = menu.project_id.clone();
        let title: gpui::SharedString = self
            .stores
            .projects
            .read(cx)
            .get(&id)
            .map(|p| p.name.clone())
            .unwrap_or_default()
            .into();
        let this = cx.entity();
        let close = this.clone();
        context_menu(
            menu.position,
            title,
            &self.menu_focus,
            vec![MenuItem {
                id: "project-menu-isolate".into(),
                icon: if menu.isolated { Icon::LockOpen } else { Icon::Lock },
                label: t(if menu.isolated {
                    "sidebar.unisolate"
                } else {
                    "sidebar.isolate"
                })
                .into(),
                on_click: std::rc::Rc::new(move |_, cx| {
                    this.update(cx, |this, cx| this.toggle_isolate(&id, cx));
                }),
            }],
            move |_, cx| close.update(cx, |this, cx| this.close_project_menu(cx)),
        )
    }

    fn close_shell_menu(&mut self, cx: &mut Context<Self>) {
        if self.shell_menu.take().is_some() {
            self.shell_menu_closed_at = Some(Instant::now());
            cx.notify();
        }
    }

    fn close_pages_menu(&mut self, cx: &mut Context<Self>) {
        if self.pages_menu {
            self.pages_menu = false;
            self.pages_menu_closed_at = Some(Instant::now());
            cx.notify();
        }
    }

    fn toggle_pages_menu(&mut self, cx: &mut Context<Self>) {
        if self.pages_menu {
            return self.close_pages_menu(cx);
        }
        if self
            .pages_menu_closed_at
            .is_none_or(|t| t.elapsed().as_millis() > REOPEN_GUARD_MS)
        {
            self.pages_menu = true;
            cx.notify();
        }
    }

    fn pages_popup(&self, hidden: &[String], cx: &mut Context<Self>) -> AnyElement {
        let mut popup = div()
            .id("pages-menu")
            .occlude()
            .absolute()
            .top(px(38.0))
            .left((theme::sidebar_width() - px(PAGES_MENU_W)) / 2.0)
            .w(px(PAGES_MENU_W))
            .p(px(8.0))
            .flex()
            .flex_col()
            .gap(px(8.0))
            .rounded(px(8.0))
            .border_1()
            .border_color(theme::border_subtle())
            .bg(theme::bg_modal())
            .text_color(theme::text())
            .on_mouse_down_out(cx.listener(|this, _, _, cx| this.close_pages_menu(cx)))
            .child(
                div()
                    .text_size(px(11.0))
                    .text_color(theme::text_muted())
                    .child(t("sidebar.pages.title")),
            );
        for (id, label) in PAGES {
            let prefs = self.stores.prefs.clone();
            popup = popup.child(checkbox(
                format!("page-toggle-{id}"),
                t(label),
                !hidden.iter().any(|h| h == id),
                move |visible, _, cx| {
                    prefs.update(cx, |p, cx| {
                        p.update(cx, |p| {
                            p.hidden_sidebar_pages.retain(|h| h != id);
                            if !visible {
                                p.hidden_sidebar_pages.push(id.to_string());
                            }
                        })
                    })
                },
            ));
        }
        deferred(popup).with_priority(1).into_any_element()
    }

    /// Shells to open in the project folder, then the agent worktrees to open the default one in.
    fn shell_menu_popup(&self, menu: &ShellMenu, cx: &mut Context<Self>) -> AnyElement {
        let remembered = self.stores.prefs.read(cx).prefs.last_shell.clone();
        let default_id = shells::pick(&menu.shells, &remembered).map(|s| s.id);
        let worktrees: Vec<(String, PathBuf)> = self
            .stores
            .sessions
            .read(cx)
            .agents(&menu.project_id)
            .filter_map(|s| {
                s.worktree
                    .as_ref()
                    .map(|w| (s.name.clone(), w.path.clone()))
            })
            .collect();
        let pid = menu.project_id.clone();

        let item = |id: gpui::SharedString| {
            div()
                .id(id)
                .flex()
                .items_center()
                .gap(px(8.0))
                .h(px(28.0))
                .px(px(8.0))
                .rounded(px(5.0))
                .text_size(px(12.5))
                .cursor_pointer()
                .hover(|s| s.bg(theme::hover()))
        };

        let mut popup = div()
            .id("shell-menu")
            .occlude()
            .absolute()
            .top(px(26.0))
            .right_0()
            .w(px(220.0))
            .p(px(4.0))
            .flex()
            .flex_col()
            .rounded(px(8.0))
            .border_1()
            .border_color(theme::border_subtle())
            .bg(theme::bg_modal())
            .text_color(theme::text())
            .on_mouse_down_out(cx.listener(|this, _, _, cx| this.close_shell_menu(cx)));

        if menu.shells.is_empty() {
            popup = popup.child(
                div()
                    .px(px(8.0))
                    .py(px(6.0))
                    .text_size(px(12.5))
                    .text_color(theme::text_muted())
                    .child(t("sidebar.no_shell")),
            );
        }
        for shell in &menu.shells {
            let (project_id, shell_id) = (pid.clone(), shell.id);
            popup = popup.child(
                item(format!("shell-{pid}-{}", shell.id).into())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.close_shell_menu(cx);
                        this.show_project(&project_id, cx);
                        sessions::begin_shell(&project_id, Some(shell_id), ShellDir::Project, cx);
                    }))
                    .child(icon(Icon::Terminal, px(14.0)).text_color(theme::text_muted()))
                    .child(div().flex_1().truncate().child(shell.label))
                    .when(default_id == Some(shell.id), |s| {
                        s.child(
                            div()
                                .text_size(px(11.0))
                                .text_color(theme::text_muted())
                                .child(t("sidebar.default_shell")),
                        )
                    }),
            );
        }
        if !worktrees.is_empty() && !menu.shells.is_empty() {
            popup = popup.child(
                div()
                    .px(px(8.0))
                    .pt(px(6.0))
                    .pb(px(2.0))
                    .text_size(px(11.0))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(theme::text_muted())
                    .child(t("sidebar.open_in_worktree")),
            );
            for (ix, (name, path)) in worktrees.into_iter().enumerate() {
                let project_id = pid.clone();
                popup = popup.child(
                    item(format!("shell-wt-{pid}-{ix}").into())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.close_shell_menu(cx);
                            this.show_project(&project_id, cx);
                            sessions::begin_shell(
                                &project_id,
                                None,
                                ShellDir::Worktree(path.clone()),
                                cx,
                            );
                        }))
                        .child(icon(Icon::Branch, px(14.0)).text_color(theme::text_muted()))
                        .child(div().flex_1().truncate().child(name)),
                );
            }
        }
        deferred(popup).with_priority(1).into_any_element()
    }

    fn add_project(&mut self, cx: &mut Context<Self>) {
        if self.picker_open {
            return;
        }
        self.picker_open = true;
        cx.spawn(async move |this, cx| {
            let picked = runtime::run(async {
                rfd::AsyncFileDialog::new()
                    .set_title(t("sidebar.add_project"))
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
            t("sidebar.remove_project.title"),
            t_n("sidebar.remove_project.body", running),
        )
        .labels(t("common.remove"), t("common.cancel"));
        confirm::ask(cx, options, move |answer, _, cx| {
            if answer == Some(true) {
                remove(cx);
            }
        });
    }

    fn commands_section(&self, project: &Project, cx: &mut Context<Self>) -> (AnyElement, f32) {
        let pid = project.id.clone();
        let fold = self.commands_folds.get(&pid).copied();
        let animating = fold.filter(|f| f.running());
        let show_body = animating.is_some() || fold.is_some_and(|f| f.open());
        let detected = self
            .stores
            .projects
            .read(cx)
            .detected
            .get(&pid)
            .cloned()
            .unwrap_or_default();
        let custom = project.commands.clone();
        let shell_label = project
            .default_shell
            .as_deref()
            .and_then(shells::label)
            .map_or_else(|| t("commands.shell_default").to_string(), str::to_string);
        let body_h = ROW_H * (1 + detected.len() + custom.len()) as f32;
        let height = ROW_H + if show_body { body_h } else { 0.0 };

        let row = |id: String| {
            div()
                .id(gpui::SharedString::from(id))
                .flex()
                .items_center()
                .gap(px(8.0))
                .h(px(ROW_H))
                .ml(px(38.0))
                .px(px(6.0))
                .rounded(px(6.0))
                .cursor_pointer()
                .hover(|s| s.bg(theme::hover()))
        };
        let tool = |id: String, glyph: Icon| {
            div()
                .id(gpui::SharedString::from(id))
                .flex_none()
                .size(px(20.0))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(4.0))
                .cursor_pointer()
                .hover(|s| s.bg(theme::hover()))
                .child(icon(glyph, px(12.0)).text_color(theme::text_muted()))
        };
        let label = |text: String| {
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_size(px(12.5))
                .text_color(theme::text())
                .child(text)
        };

        let toggle_id = pid.clone();
        let add_id = pid.clone();
        let chevron = fold_chevron(11.0, format!("commands-chevron-{pid}"), fold);
        let header = div()
            .flex()
            .items_center()
            .h(px(ROW_H))
            .ml(px(26.0))
            .px(px(6.0))
            .child(
                div()
                    .id(gpui::SharedString::from(format!("commands-toggle-{pid}")))
                    .flex()
                    .flex_1()
                    .items_center()
                    .gap(px(6.0))
                    .h_full()
                    .cursor_pointer()
                    .text_size(px(10.5))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(theme::text_muted())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.toggle_commands(&toggle_id, cx);
                    }))
                    .child(chevron)
                    .child(t("sidebar.commands")),
            )
            .child(
                tool(format!("commands-add-{pid}"), Icon::Plus).on_click(cx.listener(
                    move |this, _, _, cx| {
                        if !this.commands_open(&add_id) {
                            this.toggle_commands(&add_id, cx);
                        }
                        command_editor::open(&add_id, None, cx);
                    },
                )),
            );

        let mut body = div().flex().flex_col();
        if show_body {
            let shell_pid = pid.clone();
            body = body.child(
                row(format!("commands-shell-{pid}"))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let shells = shells::detect();
                        let current = this
                            .stores
                            .projects
                            .read(cx)
                            .get(&shell_pid)
                            .and_then(|p| p.default_shell.clone());
                        let next =
                            shells::next(&shells, current.as_deref()).map(|s| s.id.to_string());
                        this.stores
                            .projects
                            .update(cx, |p, cx| p.set_default_shell(&shell_pid, next, cx));
                    }))
                    .child(icon(Icon::Terminal, px(13.0)).text_color(theme::text_muted()))
                    .child(label(
                        t_args("commands.shell_row", &[("shell", &shell_label)]).to_string(),
                    )),
            );
            for (ix, d) in detected.into_iter().enumerate() {
                let (run_pid, line) = (pid.clone(), d.command);
                body = body.child(
                    row(format!("command-detected-{pid}-{ix}"))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.show_project(&run_pid, cx);
                            sessions::begin_command(&run_pid, &line, None, cx);
                        }))
                        .child(icon(Icon::Play, px(12.0)).text_color(theme::text_muted()))
                        .child(label(d.label)),
                );
            }
            for c in custom {
                let (run_pid, edit_pid, del_pid) = (pid.clone(), pid.clone(), pid.clone());
                let (line, shell) = (c.command.clone(), c.shell.clone());
                let (del_id, cid) = (c.id.clone(), c.id.clone());
                let edited = c.clone();
                body = body.child(
                    row(format!("command-custom-{pid}-{cid}"))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.show_project(&run_pid, cx);
                            sessions::begin_command(&run_pid, &line, shell.as_deref(), cx);
                        }))
                        .child(icon(Icon::Play, px(12.0)).text_color(theme::accent()))
                        .child(label(c.name))
                        .child(tool(format!("command-edit-{cid}"), Icon::Pencil).on_click(
                            cx.listener(move |_, _, _, cx| {
                                cx.stop_propagation();
                                command_editor::open(&edit_pid, Some(&edited), cx);
                            }),
                        ))
                        .child(tool(format!("command-delete-{cid}"), Icon::Trash).on_click(
                            cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.stores
                                    .projects
                                    .update(cx, |p, cx| p.delete_command(&del_pid, &del_id, cx));
                            }),
                        )),
                );
            }
        }
        let body = match animating {
            Some(f) => div()
                .overflow_hidden()
                .child(body)
                .with_animation(
                    (
                        gpui::SharedString::from(format!("commands-fold-{pid}")),
                        f.id,
                    ),
                    Animation::new(f.duration).with_easing(ease_out_cubic),
                    move |el, t| el.h(px(body_h * (f.from + (f.to - f.from) * t))),
                )
                .into_any_element(),
            None => body.into_any_element(),
        };
        let section = div().flex().flex_col().child(header).child(body);
        (section.into_any_element(), height)
    }

    fn project_block(&self, project: &Project, cx: &mut Context<Self>) -> gpui::AnyElement {
        let projects = self.stores.projects.read(cx);
        let selected = projects.selected.as_deref() == Some(project.id.as_str());
        let fold = self.folds.get(&project.id).copied();
        let sessions: Vec<_> = self
            .stores
            .sessions
            .read(cx)
            .agents(&project.id)
            .map(|s| (s.id.clone(), s.name.clone(), s.status, s.worktree.is_some()))
            .collect();
        let terminals: Vec<_> = self
            .stores
            .sessions
            .read(cx)
            .for_project(&project.id)
            .filter(|s| s.kind == SessionKind::Shell)
            .map(|s| (s.id.clone(), s.name.clone()))
            .collect();
        let menu_open = self
            .shell_menu
            .as_ref()
            .is_some_and(|m| m.project_id == project.id);
        let active = self.stores.sessions.read(cx).active.clone();

        let (toggle_id, select_id, new_id, menu_id, remove_id) = (
            project.id.clone(),
            project.id.clone(),
            project.id.clone(),
            project.id.clone(),
            project.id.clone(),
        );

        let skills_active = self.route == Route::Skills(project.id.clone());
        let docs_active =
            matches!(&self.route, Route::Docs(id) | Route::DocPreview(id, _) if *id == project.id);
        let git_active = self.route == Route::Git(project.id.clone());
        let git_count = self.stores.git.read(cx).count(&project.id);
        let github_active = self.route == Route::Github(project.id.clone());
        let github_id = project.id.clone();
        let history_active = self.route == Route::SessionHistory(project.id.clone());
        let on_sessions = self.route == Route::Sessions;
        let skills_id = project.id.clone();
        let docs_id = project.id.clone();
        let git_id = project.id.clone();
        let history_id = project.id.clone();
        let workflows_active = self.route == Route::Workflows(project.id.clone());
        let workflows_id = project.id.clone();
        let graphify_active = self.route == Route::Graphify(project.id.clone());
        let graphify_id = project.id.clone();

        let is_global = project.is_global();
        let display_name = if is_global {
            t("sidebar.global").to_string()
        } else {
            project.name.clone()
        };
        let compact = self.compact;
        let hidden = self
            .stores
            .prefs
            .read(cx)
            .prefs
            .hidden_sidebar_pages
            .clone();
        let shown = |id: &str| {
            !compact && (!is_global || id == "skills") && !hidden.iter().any(|h| h == id)
        };
        let page_rows = PAGES
            .iter()
            .filter(|(id, _)| *id != "commands" && shown(id))
            .count() as f32;
        let commands = shown("commands").then(|| self.commands_section(project, cx));
        let commands_h = commands.as_ref().map_or(0.0, |(_, h)| *h);

        let full_h = ROW_H * page_rows
            + commands_h
            + if sessions.is_empty() {
                0.0
            } else {
                SECTION_HEADER_H + ROW_H * sessions.len() as f32
            }
            + if terminals.is_empty() {
                0.0
            } else {
                SECTION_HEADER_H + ROW_H * terminals.len() as f32
            };
        let animating = fold.filter(|f| f.running());
        let chevron = fold_chevron(14.0, format!("fold-chevron-{}", project.id), fold);
        let show_list = self.compact || animating.is_some() || fold.is_some_and(|f| f.open());
        let animating = animating.filter(|_| !self.compact);

        let (drag_id, drag_name) = (project.id.clone(), display_name.clone());
        let (move_id, drop_id) = (project.id.clone(), project.id.clone());
        let hint = self
            .drop_hint
            .as_ref()
            .filter(|(t, _)| *t == project.id)
            .map(|(_, after)| *after);

        let context_open = self
            .project_menu
            .as_ref()
            .is_some_and(|m| m.project_id == project.id);
        let header = div()
            .id(gpui::SharedString::from(format!("header-{}", project.id)))
            .relative()
            .flex()
            .items_center()
            .h(px(32.0))
            .px(px(6.0))
            .rounded(px(6.0))
            .when(selected || context_open, |s| s.bg(theme::hover()))
            .when(!is_global, |s| {
                let menu_project = project.clone();
                s.on_mouse_down(
                    gpui::MouseButton::Right,
                    cx.listener(move |this, e: &gpui::MouseDownEvent, window, cx| {
                        this.open_project_menu(&menu_project, e.position, window, cx);
                    }),
                )
            })
            .when_some(
                self.project_menu
                    .as_ref()
                    .filter(|m| m.project_id == project.id),
                |s, m| s.child(self.project_menu_popup(m, cx)),
            )
            .on_drag(
                DraggedProject {
                    id: drag_id,
                    name: drag_name,
                },
                |d, _, _, cx| cx.new(|_| DragGhost(d.name.clone())),
            )
            .on_drag_move(cx.listener(
                move |this, ev: &gpui::DragMoveEvent<DraggedProject>, _, cx| {
                    let dragging_self = ev.drag(cx).id == move_id;
                    let inside = ev.bounds.contains(&ev.event.position);
                    let next = (inside && !dragging_self)
                        .then(|| (move_id.clone(), ev.event.position.y > ev.bounds.center().y));
                    if next.is_some() {
                        if this.drop_hint != next {
                            this.drop_hint = next;
                            cx.notify();
                        }
                    } else if this.drop_hint.as_ref().is_some_and(|(t, _)| *t == move_id) {
                        this.drop_hint = None;
                        cx.notify();
                    }
                },
            ))
            .on_drop(cx.listener(move |this, dragged: &DraggedProject, _, cx| {
                if let Some((target, after)) = this.drop_hint.take()
                    && target == drop_id
                {
                    this.stores
                        .projects
                        .update(cx, |p, cx| p.move_project(&dragged.id, &target, after, cx));
                }
                cx.notify();
            }))
            .when_some(hint, |s, after| {
                s.child(
                    div()
                        .absolute()
                        .left_0()
                        .right_0()
                        .h(px(2.0))
                        .rounded(px(1.0))
                        .bg(theme::accent())
                        .when(after, |l| l.bottom(px(-1.0)))
                        .when(!after, |l| l.top(px(-1.0))),
                )
            })
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
                        this.toggle_fold(&toggle_id, cx);
                    }))
                    .child(chevron),
            )
            .when(!is_global && self.is_isolated(project), |s| {
                s.child(
                    icon(Icon::Lock, px(12.0))
                        .mr(px(4.0))
                        .text_color(theme::text_muted()),
                )
            })
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
                    .child(
                        icon(if is_global { Icon::Globe } else { Icon::Folder }, px(16.0))
                            .text_color(theme::text_muted()),
                    )
                    .child(
                        div()
                            .truncate()
                            .text_size(px(13.0))
                            .text_color(theme::text())
                            .child(display_name),
                    ),
            )
            .child(
                div()
                    .relative()
                    .flex()
                    .flex_none()
                    .items_center()
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
                                "new-terminal-{}",
                                project.id.clone()
                            )))
                            .flex_none()
                            .w(px(16.0))
                            .h(px(24.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(4.0))
                            .cursor_pointer()
                            .hover(|s| s.bg(theme::hover()))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.toggle_shell_menu(&menu_id, cx)
                            }))
                            .child(
                                icon(Icon::ChevronDown, px(12.0)).text_color(theme::text_muted()),
                            ),
                    )
                    .children(
                        self.shell_menu
                            .as_ref()
                            .filter(|_| menu_open)
                            .map(|m| self.shell_menu_popup(m, cx)),
                    ),
            )
            .when(!is_global, |s| {
                s.child(
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
                )
            });

        let list = div()
            .flex()
            .flex_col()
            .flex_none()
            .when(shown("skills"), |s| {
                s.child(
                    div()
                        .id(gpui::SharedString::from(format!("skills-{}", project.id)))
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .h(px(ROW_H))
                        .ml(px(26.0))
                        .px(px(6.0))
                        .rounded(px(6.0))
                        .cursor_pointer()
                        .when(skills_active, |s| s.bg(theme::accent_soft()))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.stores
                                .projects
                                .update(cx, |p, cx| p.select(&skills_id, cx));
                            cx.emit(Navigate(Route::Skills(skills_id.clone())));
                        }))
                        .child(icon(Icon::Puzzle, px(13.0)).text_color(theme::text_muted()))
                        .child(
                            div()
                                .text_size(px(12.5))
                                .text_color(theme::text())
                                .child(t("sidebar.skills")),
                        ),
                )
            })
            .when(shown("docs"), |s| {
                s.child(
                    div()
                        .id(gpui::SharedString::from(format!("docs-{}", project.id)))
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .h(px(ROW_H))
                        .ml(px(26.0))
                        .px(px(6.0))
                        .rounded(px(6.0))
                        .cursor_pointer()
                        .when(docs_active, |s| s.bg(theme::accent_soft()))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.stores
                                .projects
                                .update(cx, |p, cx| p.select(&docs_id, cx));
                            cx.emit(Navigate(Route::Docs(docs_id.clone())));
                        }))
                        .child(icon(Icon::File, px(13.0)).text_color(theme::text_muted()))
                        .child(
                            div()
                                .text_size(px(12.5))
                                .text_color(theme::text())
                                .child(t("sidebar.docs")),
                        ),
                )
            })
            .when(shown("git"), |s| {
                s.child(
                    div()
                        .id(gpui::SharedString::from(format!("git-{}", project.id)))
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .h(px(ROW_H))
                        .ml(px(26.0))
                        .px(px(6.0))
                        .rounded(px(6.0))
                        .cursor_pointer()
                        .when(git_active, |s| s.bg(theme::accent_soft()))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.stores
                                .projects
                                .update(cx, |p, cx| p.select(&git_id, cx));
                            cx.emit(Navigate(Route::Git(git_id.clone())));
                        }))
                        .child(icon(Icon::Branch, px(13.0)).text_color(theme::text_muted()))
                        .child(
                            div()
                                .flex_1()
                                .text_size(px(12.5))
                                .text_color(theme::text())
                                .child(t("sidebar.git")),
                        )
                        .when(git_count > 0, |s| {
                            s.child(
                                div()
                                    .px(px(6.0))
                                    .rounded(px(8.0))
                                    .bg(theme::accent_soft())
                                    .text_size(px(10.5))
                                    .text_color(theme::accent())
                                    .child(git_count.to_string()),
                            )
                        }),
                )
            })
            .when(shown("github"), |s| {
                s.child(
                    div()
                        .id(gpui::SharedString::from(format!("github-{}", project.id)))
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .h(px(ROW_H))
                        .ml(px(26.0))
                        .px(px(6.0))
                        .rounded(px(6.0))
                        .cursor_pointer()
                        .when(github_active, |s| s.bg(theme::accent_soft()))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.stores
                                .projects
                                .update(cx, |p, cx| p.select(&github_id, cx));
                            cx.emit(Navigate(Route::Github(github_id.clone())));
                        }))
                        .child(icon(Icon::Github, px(13.0)).text_color(theme::text_muted()))
                        .child(
                            div()
                                .text_size(px(12.5))
                                .text_color(theme::text())
                                .child(t("sidebar.github")),
                        ),
                )
            })
            .when(shown("history"), |s| {
                s.child(
                    div()
                        .id(gpui::SharedString::from(format!("history-{}", project.id)))
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .h(px(ROW_H))
                        .ml(px(26.0))
                        .px(px(6.0))
                        .rounded(px(6.0))
                        .cursor_pointer()
                        .when(history_active, |s| s.bg(theme::accent_soft()))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.stores
                                .projects
                                .update(cx, |p, cx| p.select(&history_id, cx));
                            cx.emit(Navigate(Route::SessionHistory(history_id.clone())));
                        }))
                        .child(icon(Icon::History, px(13.0)).text_color(theme::text_muted()))
                        .child(
                            div()
                                .text_size(px(12.5))
                                .text_color(theme::text())
                                .child(t("sidebar.history")),
                        ),
                )
            })
            .when(shown("workflows"), |s| {
                s.child(
                    div()
                        .id(gpui::SharedString::from(format!(
                            "workflows-{}",
                            project.id
                        )))
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .h(px(ROW_H))
                        .ml(px(26.0))
                        .px(px(6.0))
                        .rounded(px(6.0))
                        .cursor_pointer()
                        .when(workflows_active, |s| s.bg(theme::accent_soft()))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.stores
                                .projects
                                .update(cx, |p, cx| p.select(&workflows_id, cx));
                            cx.emit(Navigate(Route::Workflows(workflows_id.clone())));
                        }))
                        .child(icon(Icon::Play, px(13.0)).text_color(theme::text_muted()))
                        .child(
                            div()
                                .text_size(px(12.5))
                                .text_color(theme::text())
                                .child(t("sidebar.workflows")),
                        ),
                )
            })
            .when(shown("graphify"), |s| {
                s.child(
                    div()
                        .id(gpui::SharedString::from(format!("graphify-{}", project.id)))
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .h(px(ROW_H))
                        .ml(px(26.0))
                        .px(px(6.0))
                        .rounded(px(6.0))
                        .cursor_pointer()
                        .when(graphify_active, |s| s.bg(theme::accent_soft()))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.stores
                                .projects
                                .update(cx, |p, cx| p.select(&graphify_id, cx));
                            cx.emit(Navigate(Route::Graphify(graphify_id.clone())));
                        }))
                        .child(icon(Icon::Globe, px(13.0)).text_color(theme::text_muted()))
                        .child(
                            div()
                                .text_size(px(12.5))
                                .text_color(theme::text())
                                .child(t("sidebar.graphify")),
                        ),
                )
            })
            .children(commands.map(|(el, _)| el))
            .when(!compact && !sessions.is_empty(), |s| {
                s.child(section_header(t("sidebar.agents")))
            })
            .children(sessions.into_iter().map(|(id, name, status, worktree)| {
                let (pick_id, close_id) = (id.clone(), id.clone());
                let is_active = on_sessions && active.as_deref() == Some(id.as_str());
                div()
                    .flex()
                    .items_center()
                    .h(px(ROW_H))
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
                                    icon(Icon::Branch, px(12.0)).text_color(theme::text_muted()),
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
                            .on_click(cx.listener(move |_, _, _, cx| {
                                sessions::request_close(&close_id, cx);
                            }))
                            .child(icon(Icon::Close, px(12.0)).text_color(theme::text_muted())),
                    )
            }))
            .when(!compact && !terminals.is_empty(), |s| {
                s.child(section_header(t("sidebar.terminals")))
            })
            .children(terminals.into_iter().map(|(id, name)| {
                let (pick_id, close_id) = (id.clone(), id.clone());
                let is_active = on_sessions && active.as_deref() == Some(id.as_str());
                div()
                    .flex()
                    .items_center()
                    .h(px(ROW_H))
                    .ml(px(26.0))
                    .px(px(6.0))
                    .rounded(px(6.0))
                    .when(is_active, |s| s.bg(theme::accent_soft()))
                    .child(
                        div()
                            .id(gpui::SharedString::from(format!("terminal-{id}")))
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
                            .child(icon(Icon::Terminal, px(13.0)).text_color(theme::text_muted()))
                            .child(
                                div()
                                    .truncate()
                                    .text_size(px(12.5))
                                    .text_color(theme::text())
                                    .child(name),
                            ),
                    )
                    .child(
                        div()
                            .id(gpui::SharedString::from(format!("close-terminal-{id}")))
                            .flex_none()
                            .size(px(20.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(4.0))
                            .cursor_pointer()
                            .hover(|s| s.bg(theme::hover()))
                            .on_click(cx.listener(move |_, _, _, cx| {
                                sessions::request_close(&close_id, cx);
                            }))
                            .child(icon(Icon::Close, px(12.0)).text_color(theme::text_muted())),
                    )
            }));

        div()
            .flex()
            .flex_col()
            .when(!compact, |s| s.child(header))
            .when(show_list, |s| {
                let clip = div().flex().flex_col().overflow_hidden().child(list);
                s.child(match animating {
                    Some(f) => clip
                        .with_animation(
                            (
                                gpui::SharedString::from(format!("fold-{}", project.id)),
                                f.id,
                            ),
                            Animation::new(f.duration).with_easing(ease_out_cubic),
                            move |el, t| el.h(px(full_h * (f.from + (f.to - f.from) * t))),
                        )
                        .into_any_element(),
                    None => clip.into_any_element(),
                })
            })
            .into_any_element()
    }
}

impl Sidebar {
    /// The "Modify Agentory" group, kept apart from the projects. `None` unless self-build is on.
    fn modify_block(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.stores.prefs.read(cx).prefs.self_build_enabled {
            return None;
        }
        let rows: Vec<_> = self
            .stores
            .sessions
            .read(cx)
            .modifying()
            .map(|s| (s.id.clone(), s.name.clone(), s.status))
            .collect();
        let active = self.stores.sessions.read(cx).active.clone();
        let on_sessions = self.route == Route::Sessions;
        let update = self
            .stores
            .selfbuild
            .read(cx)
            .update_available()
            .map(str::to_string);
        Some(
            div()
                .flex()
                .flex_col()
                .pb(px(6.0))
                .mb(px(4.0))
                .border_b_1()
                .border_color(theme::border_subtle())
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .h(px(30.0))
                        .px(px(6.0))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(8.0))
                                .child(icon(Icon::Pencil, px(14.0)).text_color(theme::text_muted()))
                                .child(
                                    div()
                                        .text_size(px(11.0))
                                        .font_weight(gpui::FontWeight::SEMIBOLD)
                                        .text_color(theme::text_muted())
                                        .child(t("sidebar.modify")),
                                ),
                        )
                        .child(
                            div()
                                .id("new-modify-session")
                                .flex_none()
                                .size(px(24.0))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded(px(4.0))
                                .cursor_pointer()
                                .hover(|s| s.bg(theme::hover()))
                                .on_click(cx.listener(|_, _, _, cx| {
                                    cx.emit(Navigate(Route::Sessions));
                                    sessions::begin_modify(cx);
                                }))
                                .child(icon(Icon::Plus, px(15.0)).text_color(theme::text())),
                        ),
                )
                .when_some(update, |s, tag| {
                    s.child(
                        div()
                            .id("upstream-update-available")
                            .ml(px(26.0))
                            .px(px(6.0))
                            .h(px(ROW_H))
                            .flex()
                            .items_center()
                            .rounded(px(6.0))
                            .cursor_pointer()
                            .hover(|s| s.bg(theme::hover()))
                            .text_size(px(12.0))
                            .text_color(theme::warning())
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(Navigate(Route::Settings))))
                            .child(t_args("selfbuild.upstream.available", &[("tag", &tag)])),
                    )
                })
                .children(rows.into_iter().map(|(id, name, status)| {
                    let (pick_id, close_id, review_id) = (id.clone(), id.clone(), id.clone());
                    let is_active = on_sessions && active.as_deref() == Some(id.as_str());
                    div()
                        .flex()
                        .items_center()
                        .h(px(ROW_H))
                        .ml(px(26.0))
                        .px(px(6.0))
                        .rounded(px(6.0))
                        .when(is_active, |s| s.bg(theme::accent_soft()))
                        .child(
                            div()
                                .id(gpui::SharedString::from(format!("modify-{id}")))
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
                                ),
                        )
                        .child(
                            div()
                                .id(gpui::SharedString::from(format!("review-modify-{id}")))
                                .flex_none()
                                .size(px(20.0))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded(px(4.0))
                                .cursor_pointer()
                                .hover(|s| s.bg(theme::hover()))
                                .on_click(cx.listener(move |_, _, _, cx| {
                                    cx.emit(Navigate(Route::Review(review_id.clone())));
                                }))
                                .child(
                                    icon(Icon::Branch, px(13.0)).text_color(theme::text_muted()),
                                ),
                        )
                        .child(
                            div()
                                .id(gpui::SharedString::from(format!("close-modify-{id}")))
                                .flex_none()
                                .size(px(20.0))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded(px(4.0))
                                .cursor_pointer()
                                .hover(|s| s.bg(theme::hover()))
                                .on_click(cx.listener(move |_, _, _, cx| {
                                    sessions::request_close(&close_id, cx);
                                }))
                                .child(icon(Icon::Close, px(12.0)).text_color(theme::text_muted())),
                        )
                }))
                .into_any_element(),
        )
    }
}

impl Render for Sidebar {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let projects: Vec<Project> = self.stores.projects.read(cx).items.clone();
        let modify = self.modify_block(cx);
        let blocks: Vec<_> = projects.iter().map(|p| self.project_block(p, cx)).collect();
        let empty = projects.iter().all(Project::is_global);
        let pages_popup = (self.pages_menu && !self.compact).then(|| {
            let hidden = self
                .stores
                .prefs
                .read(cx)
                .prefs
                .hidden_sidebar_pages
                .clone();
            self.pages_popup(&hidden, cx)
        });

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
                    .relative()
                    .items_center()
                    .justify_between()
                    .h(px(40.0))
                    .px(px(12.0))
                    .child(
                        div()
                            .text_size(px(11.0))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(theme::text_muted())
                            .child(t("sidebar.projects")),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(2.0))
                            .when(!self.compact, |s| {
                                s.child(
                                    div()
                                        .id("pages-toggle")
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .size(px(26.0))
                                        .rounded(px(6.0))
                                        .cursor_pointer()
                                        .when(self.pages_menu, |s| s.bg(theme::accent_soft()))
                                        .hover(|s| s.bg(theme::hover()))
                                        .on_click(
                                            cx.listener(|this, _, _, cx| {
                                                this.toggle_pages_menu(cx)
                                            }),
                                        )
                                        .child(icon(Icon::Eye, px(16.0)).text_color(theme::text())),
                                )
                            })
                            .child(
                                div()
                                    .id("compact-toggle")
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .size(px(26.0))
                                    .rounded(px(6.0))
                                    .cursor_pointer()
                                    .when(self.compact, |s| s.bg(theme::accent_soft()))
                                    .hover(|s| s.bg(theme::hover()))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.compact = !this.compact;
                                        cx.notify();
                                    }))
                                    .child(icon(Icon::Rows, px(16.0)).text_color(theme::text())),
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
                                    .child(
                                        icon(Icon::FolderPlus, px(16.0)).text_color(theme::text()),
                                    ),
                            ),
                    )
                    .children(pages_popup),
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
                    .children(modify)
                    .children(blocks)
                    .when(empty, |s| {
                        s.child(
                            div()
                                .px(px(8.0))
                                .py(px(12.0))
                                .text_size(px(12.5))
                                .text_color(theme::text_muted())
                                .child(t("sidebar.empty_hint")),
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
                    .child(t("sidebar.made_with"))
                    .child(icon(Icon::Heart, px(11.0)).text_color(gpui::rgb(0xe5484d)))
                    .child(t("sidebar.made_by")),
            )
    }
}
