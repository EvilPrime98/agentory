//! Root entity: routing, startup sequence, shell chrome (header, sidebar), the
//! global overlays (toasts, confirm dialog, app loader) and the close guard.

use std::time::{Duration, Instant};

use gpui::{
    Animation, AnimationExt as _, AnyView, App, AppContext as _, Context, Entity, FocusHandle,
    Focusable, InteractiveElement, IntoElement, ParentElement, Render, Styled, Subscription,
    Window, actions, div, prelude::*, px,
};

use crate::backend::spm;
use crate::i18n::{t, t_n};
use crate::platform::pty;
use crate::route::{GoBack, History, Navigate, Route, window_title};
use crate::runtime;
use crate::state::{Stores, sessions};
use crate::ui::app_loader::app_loader;
use crate::ui::confirm::{self, ConfirmHost, ConfirmOptions};
use crate::ui::pages::docs::DocsPage;
use crate::ui::pages::review::ReviewPage;
use crate::ui::pages::sessions::SessionsPage;
use crate::ui::pages::settings::SettingsPage;
use crate::ui::pages::skill_editor::SkillEditorPage;
use crate::ui::pages::skills::SkillsPage;
use crate::ui::shell::header::header;
use crate::ui::shell::sidebar::Sidebar;
use crate::ui::shell::usage_pill::UsagePill;
use crate::ui::theme;
use crate::ui::toast::{self, ToastHost};

actions!(
    agentory,
    [
        ToggleFullscreen,
        ToggleSidebar,
        OpenSkills,
        RequestClose,
        OpenTerminal,
        NextSession,
        PrevSession
    ]
);

pub fn bind_keys(cx: &mut App) {
    // Only combos a terminal does not need: everything else goes to the PTY.
    cx.bind_keys([
        gpui::KeyBinding::new("f11", ToggleFullscreen, None),
        gpui::KeyBinding::new("ctrl-alt-b", ToggleSidebar, None),
        gpui::KeyBinding::new("ctrl-alt-s", OpenSkills, None),
        gpui::KeyBinding::new("ctrl-alt-t", OpenTerminal, None),
        gpui::KeyBinding::new("ctrl-tab", NextSession, None),
        gpui::KeyBinding::new("ctrl-shift-tab", PrevSession, None),
        #[cfg(target_os = "macos")]
        gpui::KeyBinding::new("ctrl-cmd-f", ToggleFullscreen, None),
    ]);
}

/// What the content area shows.
enum Page {
    Sessions(Entity<SessionsPage>),
    Settings(Entity<SettingsPage>),
    Skills(Entity<SkillsPage>),
    Docs(Entity<DocsPage>),
    SkillEditor(Entity<SkillEditorPage>),
    Review(Entity<ReviewPage>),
}

const PANEL_OPEN: Duration = Duration::from_millis(220);
const PANEL_CLOSE: Duration = Duration::from_millis(180);

fn ease_out_cubic(t: f32) -> f32 {
    1.0 - (1.0 - t).powi(3)
}

fn ease_in_cubic(t: f32) -> f32 {
    t * t * t
}

/// A slide of the sidebar between `from` and `to`
/// (0 = hidden, 1 = fully shown).
#[derive(Clone, Copy)]
struct PanelAnim {
    id: usize,
    from: f32,
    to: f32,
    started: Instant,
    duration: Duration,
}

impl PanelAnim {
    /// Starts a slide toward the opposite of `was_open`, reversing from wherever `prev` currently is
    /// so a mid-animation toggle does not jump.
    fn toggle(prev: Option<Self>, was_open: bool) -> Self {
        let from = match prev {
            Some(a) => a.progress(),
            None if was_open => 1.0,
            None => 0.0,
        };
        let to = if was_open { 0.0 } else { 1.0 };
        let full = if was_open { PANEL_CLOSE } else { PANEL_OPEN };
        Self {
            id: prev.map_or(0, |a| a.id + 1),
            from,
            to,
            started: Instant::now(),
            duration: full
                .mul_f32((to - from).abs())
                .max(Duration::from_millis(1)),
        }
    }

    /// Re-renders once the slide is over so a closed panel is unmounted.
    fn notify_when_done(&self, cx: &mut Context<AppRoot>) {
        let wait = self.duration + Duration::from_millis(16);
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(wait).await;
            this.update(cx, |_, cx| cx.notify()).ok();
        })
        .detach();
    }

    fn easing(&self) -> fn(f32) -> f32 {
        if self.to > self.from {
            ease_out_cubic
        } else {
            ease_in_cubic
        }
    }

    fn running(&self) -> bool {
        self.started.elapsed() < self.duration
    }

    fn progress(&self) -> f32 {
        let t = (self.started.elapsed().as_secs_f32() / self.duration.as_secs_f32()).min(1.0);
        self.from + (self.to - self.from) * self.easing()(t)
    }
}

pub struct AppRoot {
    history: History,
    stores: Stores,
    sidebar: Entity<Sidebar>,
    usage_pill: Entity<UsagePill>,
    toasts: Entity<ToastHost>,
    confirm: Entity<ConfirmHost>,
    page: Page,
    _page_subs: Vec<Subscription>,
    _subs: Vec<Subscription>,
    /// The startup checks are still running: show the AppLoader.
    loading: bool,
    /// Dev aid: `AGENTORY_OPEN=settings` opens a page once startup finishes.
    pending_route: Option<Route>,
    focus: FocusHandle,
    /// The close guard has been answered: the next close request goes through.
    close_confirmed: bool,
    close_check_pending: bool,
    sidebar_anim: Option<PanelAnim>,
    /// Last title given to the OS window, so a language switch can refresh it.
    window_title: String,
}

impl AppRoot {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        window.focus(&focus, cx);

        let stores = Stores::new(cx);
        let toasts = toast::init(cx);
        let confirm = confirm::init(cx);
        cx.set_global(stores.clone());

        let sidebar = cx.new(|cx| Sidebar::new(stores.clone(), cx));
        let usage_pill = cx.new(|cx| UsagePill::new(stores.clone(), cx));
        let sessions_store = stores.sessions.clone();
        let subs = vec![
            cx.subscribe_in(&sidebar, window, |this, _, ev: &Navigate, window, cx| {
                this.navigate(ev.0.clone(), window, cx)
            }),
            cx.observe(&stores.prefs, |_, _, cx| cx.notify()),
            // The skills store follows the selected project; a Skills page whose project is gone
            // has nothing left to show.
            cx.observe_in(&stores.projects, window, |this, _, window, cx| {
                this.sync_skills_project(cx);
                if let Route::Skills(id) | Route::Docs(id) = this.history.current() {
                    if this.stores.projects.read(cx).get(id).is_none() {
                        this.history.back_or_home();
                        this.route_changed(window, cx);
                    }
                }
            }),
            cx.observe_window_activation(window, |this, window, cx| {
                let active = window.is_window_active();
                this.stores
                    .sessions
                    .update(cx, |s, _| s.app_active = active);
            }),
            // Child processes must not outlive the app.
            cx.on_app_quit(move |_, cx| {
                sessions_store.update(cx, |s, cx| s.kill_all(cx));
                async {}
            }),
        ];

        let mut this = Self {
            history: History::default(),
            stores,
            sidebar,
            usage_pill,
            toasts,
            confirm,
            page: Page::Sessions(cx.new(|cx| SessionsPage::new(cx.global::<Stores>().clone(), cx))),
            _page_subs: Vec::new(),
            _subs: subs,
            loading: true,
            pending_route: dev_route(),
            focus,
            close_confirmed: false,
            close_check_pending: false,
            sidebar_anim: None,
            window_title: String::new(),
        };
        // Closing with running sessions asks first (`on_window_should_close` is synchronous, so the
        // answer arrives later and the window closes itself).
        let root = cx.entity();
        window.on_window_should_close(cx, move |window, cx| {
            root.update(cx, |root, cx| root.guard_close(window, cx))
        });
        this.sync_window_title(window);
        this.build_page(window, cx);
        this.sync_skills_project(cx);
        this.begin_startup(cx);
        this
    }

    pub fn route(&self) -> &Route {
        self.history.current()
    }

    fn sync_skills_project(&self, cx: &mut Context<Self>) {
        let path = self
            .stores
            .projects
            .read(cx)
            .selected_project()
            .map(|p| p.path.clone());
        self.stores
            .skills
            .update(cx, |s, cx| s.set_project(path, cx));
    }

    /// Check the external tools in parallel (failures are reported, they do not abort), then drop
    /// the loader.
    fn begin_startup(&mut self, cx: &mut Context<Self>) {
        let program = self.stores.prefs.read(cx).prefs.program().to_string();
        cx.spawn(async move |this, cx| {
            let (claude, spm_ok) = runtime::run(async move {
                let claude = pty::resolve_program(&program);
                (claude, spm::available().await)
            })
            .await;
            this.update(cx, |this, cx| {
                this.loading = false;
                cx.notify();
                match &claude {
                    Some(path) => tracing::info!("claude: {}", path.display()),
                    None => {
                        tracing::warn!("claude was not found on PATH");
                        toast::error(
                            cx,
                            t("app.claude_missing"),
                        );
                    }
                }
                if !spm_ok {
                    tracing::warn!("spm was not found on PATH");
                    toast::error(
                        cx,
                        t("app.spm_missing"),
                    );
                }
                // Dev aid: start a session in the selected project right away.
                if std::env::var_os("AGENTORY_AUTOSTART").is_some() {
                    let id = this.stores.projects.read(cx).selected.clone();
                    if let Some(id) = id {
                        sessions::begin_new(&id, cx);
                    }
                }
            })
            .ok();
        })
        .detach();
    }

    /// Close-guard decision for a close request: `true` lets the window close now. With running
    /// sessions it asks first and closes the window itself on a "yes".
    pub fn guard_close(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.close_confirmed {
            return true;
        }
        let running = self.stores.sessions.read(cx).running();
        if running == 0 {
            return true;
        }
        if self.close_check_pending {
            return false;
        }
        self.close_check_pending = true;
        let root = cx.entity();
        let handle = window.window_handle();
        let options = ConfirmOptions::new(
            t("app.close.title"),
            t_n("app.close.body", running),
        )
        .labels(t("common.close"), t("common.cancel"));
        confirm::ask(cx, options, move |answer, _, cx| {
            let close = answer == Some(true);
            root.update(cx, |root, _| {
                root.close_check_pending = false;
                root.close_confirmed = close;
            });
            if close {
                handle
                    .update(cx, |_, window, _| window.remove_window())
                    .ok();
            }
        });
        false
    }

    pub fn navigate(&mut self, route: Route, window: &mut Window, cx: &mut Context<Self>) {
        self.leave_guarded(window, cx, move |this, window, cx| {
            this.history.push(route);
            this.route_changed(window, cx);
        });
    }

    pub fn go_back(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.leave_guarded(window, cx, |this, window, cx| {
            this.history.back_or_home();
            this.route_changed(window, cx);
        });
    }

    /// Run `leave` now, or after a confirmation when the skill editor holds unsaved edits.
    fn leave_guarded(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        leave: impl FnOnce(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) {
        let dirty = matches!(&self.page, Page::SkillEditor(p) if p.read(cx).is_dirty(cx));
        if !dirty {
            return leave(self, window, cx);
        }
        let root = cx.entity();
        let handle = window.window_handle();
        let options = ConfirmOptions::new(
            t("app.unsaved.title"),
            t("app.unsaved.body"),
        )
        .labels(t("app.unsaved.discard"), t("app.unsaved.keep"));
        confirm::ask(cx, options, move |answer, _, cx| {
            if answer == Some(true) {
                handle
                    .update(cx, |_, window, cx| {
                        root.update(cx, |root, cx| leave(root, window, cx))
                    })
                    .ok();
            }
        });
    }

    /// Set the OS window title when it differs (route or language changed).
    fn sync_window_title(&mut self, window: &mut Window) {
        let title = window_title(self.history.current());
        if title != self.window_title {
            window.set_window_title(&title);
            self.window_title = title;
        }
    }

    fn route_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.sync_window_title(window);
        let route = self.history.current().clone();
        self.sidebar.update(cx, |s, cx| s.set_route(route, cx));
        self.build_page(window, cx);
        cx.notify();
    }

    /// Instantiate the view for the current route (a fresh page per visit).
    fn build_page(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let route = self.history.current().clone();
        self._page_subs.clear();
        self.page = match route {
            Route::Sessions => {
                let page = cx.new(|cx| SessionsPage::new(self.stores.clone(), cx));
                self._page_subs.push(cx.subscribe_in(
                    &page,
                    window,
                    |this, _, ev: &Navigate, window, cx| this.navigate(ev.0.clone(), window, cx),
                ));
                self._page_subs.push(cx.subscribe_in(
                    &page,
                    window,
                    |this, _, _: &GoBack, window, cx| this.go_back(window, cx),
                ));
                Page::Sessions(page)
            }
            Route::Settings => {
                let page = cx.new(|cx| SettingsPage::new(self.stores.clone(), cx));
                self._page_subs.push(cx.subscribe_in(
                    &page,
                    window,
                    |this, _, ev: &Navigate, window, cx| this.navigate(ev.0.clone(), window, cx),
                ));
                self._page_subs.push(cx.subscribe_in(
                    &page,
                    window,
                    |this, _, _: &GoBack, window, cx| this.go_back(window, cx),
                ));
                Page::Settings(page)
            }
            Route::Skills(id) => {
                self.stores.projects.update(cx, |p, cx| p.select(&id, cx));
                self.sync_skills_project(cx);
                self.stores.skills.update(cx, |s, cx| s.refresh(cx));
                let page = cx.new(|cx| SkillsPage::new(self.stores.clone(), cx));
                self._page_subs.push(cx.subscribe_in(
                    &page,
                    window,
                    |this, _, ev: &Navigate, window, cx| this.navigate(ev.0.clone(), window, cx),
                ));
                self._page_subs.push(cx.subscribe_in(
                    &page,
                    window,
                    |this, _, _: &GoBack, window, cx| this.go_back(window, cx),
                ));
                Page::Skills(page)
            }
            Route::Docs(id) => {
                self.stores.projects.update(cx, |p, cx| p.select(&id, cx));
                let page = cx.new(|cx| DocsPage::new(self.stores.clone(), id, cx));
                self._page_subs.push(cx.subscribe_in(
                    &page,
                    window,
                    |this, _, ev: &Navigate, window, cx| this.navigate(ev.0.clone(), window, cx),
                ));
                self._page_subs.push(cx.subscribe_in(
                    &page,
                    window,
                    |this, _, _: &GoBack, window, cx| this.go_back(window, cx),
                ));
                Page::Docs(page)
            }
            Route::SkillEditor(id, name) => {
                self.stores.projects.update(cx, |p, cx| p.select(&id, cx));
                self.sync_skills_project(cx);
                let page = cx.new(|cx| SkillEditorPage::new(self.stores.clone(), name, cx));
                self._page_subs.push(cx.subscribe_in(
                    &page,
                    window,
                    |this, _, _: &GoBack, window, cx| this.go_back(window, cx),
                ));
                Page::SkillEditor(page)
            }
            Route::Review(id) => {
                let page = cx.new(|cx| ReviewPage::new(self.stores.clone(), id, cx));
                self._page_subs.push(cx.subscribe_in(
                    &page,
                    window,
                    |this, _, ev: &Navigate, window, cx| this.navigate(ev.0.clone(), window, cx),
                ));
                self._page_subs.push(cx.subscribe_in(
                    &page,
                    window,
                    |this, _, _: &GoBack, window, cx| this.go_back(window, cx),
                ));
                Page::Review(page)
            }
        };
    }

    pub fn toggle_sidebar(&mut self, cx: &mut Context<Self>) {
        let was_open = !self.stores.prefs.read(cx).prefs.sidebar_collapsed;
        let anim = PanelAnim::toggle(self.sidebar_anim, was_open);
        self.sidebar_anim = Some(anim);
        self.stores
            .prefs
            .update(cx, |p, cx| p.update(cx, |p| p.sidebar_collapsed = was_open));
        anim.notify_when_done(cx);
    }

    /// The sidebar in a wrapper that animates its width and clips it. The sidebar keeps its full
    /// width, pinned to the wrapper's right edge, so it slides left and its text never rewraps.
    /// `None` when it is fully hidden.
    fn sidebar_panel(&self, open: bool) -> Option<gpui::AnyElement> {
        let width = theme::sidebar_width();
        let shell = || {
            div()
                .relative()
                .flex_none()
                .overflow_hidden()
                .w(width)
                .child(
                    div()
                        .absolute()
                        .top_0()
                        .bottom_0()
                        .right_0()
                        .w(width)
                        .flex()
                        .child(self.sidebar.clone()),
                )
        };
        match self.sidebar_anim.filter(PanelAnim::running) {
            Some(a) => Some(
                shell()
                    .with_animation(
                        ("sidebar", a.id),
                        Animation::new(a.duration).with_easing(a.easing()),
                        move |el, t| {
                            let p = a.from + (a.to - a.from) * t;
                            el.w(width * p).opacity(0.5 + 0.5 * p)
                        },
                    )
                    .into_any_element(),
            ),
            None if open => Some(shell().into_any_element()),
            None => None,
        }
    }
}

impl Focusable for AppRoot {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for AppRoot {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync_window_title(window);
        crate::selfbuild::ready::on_frame();
        if !self.loading {
            if let Some(route) = self.pending_route.take() {
                self.navigate(route, window, cx);
            }
        }
        let prefs = self.stores.prefs.read(cx).prefs.clone();
        let route = self.history.current().clone();

        let content = match &self.page {
            Page::Sessions(page) => AnyView::from(page.clone()).into_any_element(),
            Page::Settings(page) => AnyView::from(page.clone()).into_any_element(),
            Page::Skills(page) => AnyView::from(page.clone()).into_any_element(),
            Page::Docs(page) => AnyView::from(page.clone()).into_any_element(),
            Page::SkillEditor(page) => AnyView::from(page.clone()).into_any_element(),
            Page::Review(page) => AnyView::from(page.clone()).into_any_element(),
        };

        div()
            .id("app-root")
            .key_context("AppRoot")
            .track_focus(&self.focus)
            .on_action(cx.listener(|_, _: &ToggleFullscreen, window, _| window.toggle_fullscreen()))
            .on_action(cx.listener(|this, _: &ToggleSidebar, _, cx| this.toggle_sidebar(cx)))
            .on_action(cx.listener(|this, _: &OpenSkills, window, cx| {
                let selected = this.stores.projects.read(cx).selected.clone();
                if let Some(id) = selected {
                    this.navigate(Route::Skills(id), window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &OpenTerminal, window, cx| {
                // The default shell in the selected project's folder.
                let selected = this.stores.projects.read(cx).selected.clone();
                if let Some(id) = selected {
                    this.navigate(Route::Sessions, window, cx);
                    sessions::begin_shell(&id, None, sessions::ShellDir::Project, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &NextSession, _, cx| {
                this.stores.sessions.update(cx, |s, cx| s.cycle(true, cx));
            }))
            .on_action(cx.listener(|this, _: &PrevSession, _, cx| {
                this.stores.sessions.update(cx, |s, cx| s.cycle(false, cx));
            }))
            // The Linux close button goes through here so it is guarded like the OS close.
            .on_action(cx.listener(|this, _: &RequestClose, window, cx| {
                if this.guard_close(window, cx) {
                    window.remove_window();
                }
            }))
            .relative()
            .flex()
            .flex_col()
            .size_full()
            .font_family(theme::FONT)
            .text_size(px(14.0))
            .text_color(theme::text())
            .bg(theme::bg_app())
            .child(header(
                window,
                self.usage_pill.clone(),
                route == Route::Settings,
                cx,
            ))
            .child(
                div()
                    .relative()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .children(self.sidebar_panel(!prefs.sidebar_collapsed))
                    .child(content),
            )
            .child(self.toasts.clone())
            .child(self.confirm.clone())
            .when(self.loading, |s| s.child(app_loader(t("app.checking_tools"))))
    }
}

/// Dev aid: `AGENTORY_OPEN=settings|sessions`.
fn dev_route() -> Option<Route> {
    match std::env::var("AGENTORY_OPEN").ok()?.as_str() {
        "settings" => Some(Route::Settings),
        "sessions" => Some(Route::Sessions),
        _ => None,
    }
}
