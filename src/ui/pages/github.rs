use std::future::Future;
use std::path::PathBuf;

use gpui::{
    AnyElement, Context, Entity, EventEmitter, InteractiveElement, IntoElement, ParentElement,
    Point, Render, Rgba, ScrollHandle, SharedString, StatefulInteractiveElement, Styled,
    Subscription, Window, div, prelude::*, px,
};

use super::git::{MAX_DIFF_LINES, diff_view, tint};
use super::skills::{icon_button, muted_note};
use crate::backend::gh::{
    self, Check, CheckState, Comment, IssueDetail, IssueSummary, PrDetail, PrSummary, Setup,
};
use crate::backend::git::{DiffLine, LineKind, parse_diff};
use crate::backend::transcripts;
use crate::i18n::{t, t_args, t_n};
use crate::route::{GoBack, Navigate, Route};
use crate::runtime;
use crate::state::{Stores, sessions};
use crate::ui::components::button::{ButtonVariant, button, small_button};
use crate::ui::components::scrollbar::Scrollbar;
use crate::ui::components::text_area::TextArea;
use crate::ui::components::text_input::{TextInput, TextInputEvent};
use crate::ui::confirm::{self, ConfirmOptions};
use crate::ui::icons::{Icon, icon};
use crate::ui::{theme, toast};

const LEFT_PANE_W: f32 = 340.0;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Prs,
    Issues,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PrView {
    Conversation,
    Files,
    Checks,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Picker {
    Labels,
    Assignees,
}

struct Failure {
    op: String,
    output: String,
}

pub struct GithubPage {
    list_bar: Scrollbar,
    detail_bar: Scrollbar,
    stores: Stores,
    project_id: String,
    setup: Option<Setup>,
    tab: Tab,
    prs: Vec<PrSummary>,
    issues: Vec<IssueSummary>,
    list_loading: bool,
    list_error: Option<String>,
    list_gen: u64,
    pr: Option<u64>,
    issue: Option<u64>,
    pr_detail: Option<PrDetail>,
    branch_deletable: bool,
    issue_detail: Option<IssueDetail>,
    detail_error: Option<String>,
    detail_gen: u64,
    pr_view: PrView,
    diff: Vec<DiffLine>,
    diff_hidden: usize,
    diff_gen: u64,
    diff_loaded: bool,
    diff_scroll: ScrollHandle,
    comment: Entity<TextArea>,
    me: Option<String>,
    editing_issue: bool,
    edit_title: Entity<TextInput>,
    edit_body: Entity<TextArea>,
    editing_comment: Option<u64>,
    edit_comment: Entity<TextArea>,
    picker: Option<Picker>,
    repo_labels: Vec<String>,
    repo_users: Vec<String>,
    creating: bool,
    new_title: Entity<TextInput>,
    new_body: Entity<TextArea>,
    busy: bool,
    failure: Option<Failure>,
    failure_open: bool,
    _subs: Vec<Subscription>,
}

impl EventEmitter<Navigate> for GithubPage {}
impl EventEmitter<GoBack> for GithubPage {}

fn state_color(state: &str) -> Rgba {
    match state {
        "OPEN" => theme::success(),
        "MERGED" => theme::accent(),
        _ => theme::error(),
    }
}

fn state_label(state: &str, draft: bool) -> String {
    if draft && state == "OPEN" {
        return t("github.state.draft");
    }
    t(match state {
        "OPEN" => "github.state.open",
        "MERGED" => "github.state.merged",
        _ => "github.state.closed",
    })
}

fn day(iso: &str) -> &str {
    iso.get(..10).unwrap_or(iso)
}

fn byline(key: &str, author: &str, created_at: &str) -> String {
    let Some(ms) = crate::backend::spm::doctor::parse_ms(created_at) else {
        return author.to_string();
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64);
    let age = match transcripts::age(ms, now) {
        ("now", _) => t("history.age.now"),
        (unit, n) => t_n(&format!("history.age.{unit}"), n as usize),
    };
    t_args(key, &[("author", author), ("age", &age)])
}

impl GithubPage {
    pub fn new(stores: Stores, project_id: String, cx: &mut Context<Self>) -> Self {
        let comment = cx.new(TextArea::new);
        let new_title = cx.new(|cx| TextInput::new(t("github.new.title_placeholder"), cx));
        let new_body = cx.new(TextArea::new);
        let edit_title = cx.new(|cx| TextInput::new(t("github.new.title_placeholder"), cx));
        let edit_body = cx.new(TextArea::new);
        let edit_comment = cx.new(TextArea::new);
        let subs = vec![
            cx.subscribe(&edit_title, |this, _, ev: &TextInputEvent, cx| match ev {
                TextInputEvent::Submit => this.save_issue_edit(cx),
                TextInputEvent::Cancel => this.cancel_issue_edit(cx),
                TextInputEvent::Changed => {}
            }),
            cx.subscribe(&new_title, |this, _, ev: &TextInputEvent, cx| match ev {
                TextInputEvent::Submit => this.create(cx),
                TextInputEvent::Cancel => this.close_create(cx),
                TextInputEvent::Changed => {}
            }),
        ];
        let mut this = Self {
            list_bar: Scrollbar::new(),
            detail_bar: Scrollbar::new(),
            stores,
            project_id,
            setup: None,
            tab: Tab::Prs,
            prs: Vec::new(),
            issues: Vec::new(),
            list_loading: false,
            list_error: None,
            list_gen: 0,
            pr: None,
            issue: None,
            pr_detail: None,
            branch_deletable: false,
            issue_detail: None,
            detail_error: None,
            detail_gen: 0,
            pr_view: PrView::Conversation,
            diff: Vec::new(),
            diff_hidden: 0,
            diff_gen: 0,
            diff_loaded: false,
            diff_scroll: ScrollHandle::new(),
            comment,
            me: None,
            editing_issue: false,
            edit_title,
            edit_body,
            editing_comment: None,
            edit_comment,
            picker: None,
            repo_labels: Vec::new(),
            repo_users: Vec::new(),
            creating: false,
            new_title,
            new_body,
            busy: false,
            failure: None,
            failure_open: false,
            _subs: subs,
        };
        this.check_setup(cx);
        this
    }

    fn dir(&self, cx: &Context<Self>) -> Option<PathBuf> {
        self.stores.projects.read(cx).path_of(&self.project_id)
    }

    fn check_setup(&mut self, cx: &mut Context<Self>) {
        let Some(dir) = self.dir(cx) else {
            self.setup = Some(Setup::NoRemote);
            cx.notify();
            return;
        };
        self.setup = None;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let setup = runtime::run(async move { gh::setup(&dir).await }).await;
            this.update(cx, |this, cx| {
                let ready = setup == Setup::Ready;
                this.setup = Some(setup);
                if ready {
                    this.refresh(cx);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        self.load_list(cx);
        match self.tab {
            Tab::Prs if self.pr.is_some() => {
                self.load_pr(cx);
                self.clear_diff();
                if self.pr_view == PrView::Files {
                    self.load_diff(cx);
                }
            }
            Tab::Issues if self.issue.is_some() => self.load_issue(cx),
            _ => {}
        }
    }

    fn load_list(&mut self, cx: &mut Context<Self>) {
        let Some(dir) = self.dir(cx) else {
            return;
        };
        self.list_gen += 1;
        let generation = self.list_gen;
        self.list_loading = true;
        let tab = self.tab;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let (prs, issues) = match tab {
                Tab::Prs => (
                    Some(runtime::run(async move { gh::pr_list(&dir).await }).await),
                    None,
                ),
                Tab::Issues => (
                    None,
                    Some(runtime::run(async move { gh::issue_list(&dir).await }).await),
                ),
            };
            this.update(cx, |this, cx| {
                if this.list_gen != generation {
                    return;
                }
                this.list_loading = false;
                this.list_error = None;
                match prs {
                    Some(Ok(v)) => this.prs = v,
                    Some(Err(e)) => this.list_error = Some(e),
                    None => {}
                }
                match issues {
                    Some(Ok(v)) => this.issues = v,
                    Some(Err(e)) => this.list_error = Some(e),
                    None => {}
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn set_tab(&mut self, tab: Tab, cx: &mut Context<Self>) {
        if self.tab == tab {
            return;
        }
        self.tab = tab;
        self.creating = false;
        self.list_error = None;
        self.detail_error = None;
        self.load_list(cx);
        if tab == Tab::Prs && self.pr.is_some() && self.pr_detail.is_none() {
            self.load_pr(cx);
        }
        if tab == Tab::Issues && self.issue.is_some() && self.issue_detail.is_none() {
            self.load_issue(cx);
        }
        cx.notify();
    }

    fn clear_comment(&mut self, cx: &mut Context<Self>) {
        self.comment.update(cx, |c, cx| c.set_value("", cx));
    }

    fn select_pr(&mut self, number: u64, cx: &mut Context<Self>) {
        self.creating = false;
        if self.pr == Some(number) {
            return;
        }
        self.pr = Some(number);
        self.pr_detail = None;
        self.branch_deletable = false;
        self.detail_error = None;
        self.pr_view = PrView::Conversation;
        self.clear_diff();
        self.clear_comment(cx);
        self.load_pr(cx);
        cx.notify();
    }

    fn select_issue(&mut self, number: u64, cx: &mut Context<Self>) {
        self.creating = false;
        if self.issue == Some(number) {
            return;
        }
        self.issue = Some(number);
        self.issue_detail = None;
        self.detail_error = None;
        self.editing_issue = false;
        self.editing_comment = None;
        self.picker = None;
        self.clear_comment(cx);
        self.load_issue(cx);
        cx.notify();
    }

    fn load_me(&mut self, cx: &mut Context<Self>) {
        let Some(dir) = self.dir(cx) else {
            return;
        };
        cx.spawn(async move |this, cx| {
            let login = runtime::run(async move { gh::current_user(&dir).await }).await;
            this.update(cx, |this, cx| {
                if let Ok(login) = login {
                    this.me = Some(login);
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    fn toggle_picker(&mut self, picker: Picker, cx: &mut Context<Self>) {
        if self.picker == Some(picker) {
            self.picker = None;
            cx.notify();
            return;
        }
        self.picker = Some(picker);
        let loaded = match picker {
            Picker::Labels => !self.repo_labels.is_empty(),
            Picker::Assignees => !self.repo_users.is_empty(),
        };
        if !loaded && let Some(dir) = self.dir(cx) {
            cx.spawn(async move |this, cx| {
                let result = runtime::run(async move {
                    match picker {
                        Picker::Labels => gh::repo_labels(&dir).await,
                        Picker::Assignees => gh::assignable_users(&dir).await,
                    }
                })
                .await;
                this.update(cx, |this, cx| {
                    match result {
                        Ok(v) => match picker {
                            Picker::Labels => this.repo_labels = v,
                            Picker::Assignees => this.repo_users = v,
                        },
                        Err(e) => {
                            toast::error(cx, t_args("github.read_failed", &[("error", &e)]));
                        }
                    }
                    cx.notify();
                })
                .ok();
            })
            .detach();
        }
        cx.notify();
    }

    fn toggle_pick(&mut self, picker: Picker, name: String, on: bool, cx: &mut Context<Self>) {
        let Some(number) = self.issue else {
            return;
        };
        let label = t(match picker {
            Picker::Labels => "github.labels",
            Picker::Assignees => "github.assignees",
        });
        self.op(label, false, cx, move |dir| async move {
            match picker {
                Picker::Labels => gh::issue_label(&dir, number, &name, !on).await,
                Picker::Assignees => gh::issue_assignee(&dir, number, &name, !on).await,
            }
        });
    }

    fn start_issue_edit(&mut self, cx: &mut Context<Self>) {
        let Some(issue) = &self.issue_detail else {
            return;
        };
        let (title, body) = (issue.title.clone(), issue.body.clone());
        self.edit_title.update(cx, |i, cx| i.set_value(title, cx));
        self.edit_body.update(cx, |b, cx| b.set_value(&body, cx));
        self.editing_issue = true;
        cx.notify();
    }

    fn cancel_issue_edit(&mut self, cx: &mut Context<Self>) {
        self.editing_issue = false;
        cx.notify();
    }

    fn save_issue_edit(&mut self, cx: &mut Context<Self>) {
        let title = self.edit_title.read(cx).value().trim().to_string();
        if title.is_empty() {
            toast::error(cx, t("github.new.title_required"));
            return;
        }
        let body = self.edit_body.read(cx).value();
        let Some(number) = self.issue else {
            return;
        };
        self.editing_issue = false;
        self.op(t("github.edit"), false, cx, move |dir| async move {
            gh::issue_edit(&dir, number, &title, &body).await
        });
    }

    fn start_comment_edit(&mut self, id: u64, body: &str, cx: &mut Context<Self>) {
        self.edit_comment.update(cx, |b, cx| b.set_value(body, cx));
        self.editing_comment = Some(id);
        cx.notify();
    }

    fn cancel_comment_edit(&mut self, cx: &mut Context<Self>) {
        self.editing_comment = None;
        cx.notify();
    }

    fn save_comment_edit(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.editing_comment else {
            return;
        };
        let body = self.edit_comment.read(cx).value().trim().to_string();
        if body.is_empty() {
            toast::error(cx, t("github.comment.empty"));
            return;
        }
        self.editing_comment = None;
        self.op(t("github.edit"), false, cx, move |dir| async move {
            gh::comment_edit(&dir, id, &body).await
        });
    }

    fn load_pr(&mut self, cx: &mut Context<Self>) {
        let (Some(dir), Some(number)) = (self.dir(cx), self.pr) else {
            return;
        };
        self.detail_gen += 1;
        let generation = self.detail_gen;
        cx.spawn(async move |this, cx| {
            let (result, deletable) = runtime::run(async move {
                let result = gh::pr_view(&dir, number).await;
                let deletable = match &result {
                    Ok(pr) => gh::branch_deletable(&dir, pr).await,
                    Err(_) => false,
                };
                (result, deletable)
            })
            .await;
            this.update(cx, |this, cx| {
                if this.detail_gen != generation {
                    return;
                }
                this.branch_deletable = deletable;
                match result {
                    Ok(d) => {
                        this.pr_detail = Some(d);
                        this.detail_error = None;
                    }
                    Err(e) => this.detail_error = Some(e),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn load_issue(&mut self, cx: &mut Context<Self>) {
        let (Some(dir), Some(number)) = (self.dir(cx), self.issue) else {
            return;
        };
        if self.me.is_none() {
            self.load_me(cx);
        }
        self.detail_gen += 1;
        let generation = self.detail_gen;
        cx.spawn(async move |this, cx| {
            let result = runtime::run(async move { gh::issue_view(&dir, number).await }).await;
            this.update(cx, |this, cx| {
                if this.detail_gen != generation {
                    return;
                }
                match result {
                    Ok(d) => {
                        this.issue_detail = Some(d);
                        this.detail_error = None;
                    }
                    Err(e) => this.detail_error = Some(e),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn clear_diff(&mut self) {
        self.diff_gen += 1;
        self.diff.clear();
        self.diff_hidden = 0;
        self.diff_loaded = false;
        self.diff_scroll.set_offset(Point::default());
    }

    fn load_diff(&mut self, cx: &mut Context<Self>) {
        let (Some(dir), Some(number)) = (self.dir(cx), self.pr) else {
            return;
        };
        self.diff_gen += 1;
        let generation = self.diff_gen;
        self.diff_loaded = true;
        cx.spawn(async move |this, cx| {
            let text = runtime::run(async move { gh::pr_diff(&dir, number).await }).await;
            this.update(cx, |this, cx| {
                if this.diff_gen != generation {
                    return;
                }
                let text_ok = text.is_ok();
                let mut lines = match text {
                    Ok(text) => parse_diff(&text),
                    Err(e) => vec![DiffLine {
                        kind: LineKind::Note,
                        old: None,
                        new: None,
                        text: e,
                    }],
                };
                this.diff_loaded = text_ok;
                this.diff_hidden = lines.len().saturating_sub(MAX_DIFF_LINES);
                lines.truncate(MAX_DIFF_LINES);
                this.diff = lines;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn set_pr_view(&mut self, view: PrView, cx: &mut Context<Self>) {
        if self.pr_view == view {
            return;
        }
        self.pr_view = view;
        if view == PrView::Files && !self.diff_loaded {
            self.load_diff(cx);
        }
        cx.notify();
    }

    fn op<F, Fut>(&mut self, label: String, reset: bool, cx: &mut Context<Self>, f: F)
    where
        F: FnOnce(PathBuf) -> Fut,
        Fut: Future<Output = Result<(), String>> + Send + 'static,
    {
        let Some(dir) = self.dir(cx) else {
            return;
        };
        if self.busy {
            toast::error(cx, t("github.busy"));
            return;
        }
        self.busy = true;
        self.failure = None;
        cx.notify();
        let fut = f(dir);
        cx.spawn(async move |this, cx| {
            let result = runtime::run(fut).await;
            this.update(cx, |this, cx| {
                this.busy = false;
                match result {
                    Ok(()) => {
                        toast::success(cx, t_args("github.done", &[("op", &label)]));
                        if reset {
                            this.clear_comment(cx);
                            this.close_create(cx);
                        }
                    }
                    Err(output) => {
                        toast::error(cx, t_args("github.failed", &[("op", &label)]));
                        this.failure = Some(Failure { op: label, output });
                        this.failure_open = false;
                    }
                }
                this.refresh(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn ask(
        &mut self,
        options: ConfirmOptions,
        cx: &mut Context<Self>,
        run: impl FnOnce(&mut Self, &mut Context<Self>) + 'static,
    ) {
        let page = cx.entity();
        confirm::ask(cx, options, move |answer, _, cx| {
            if answer == Some(true) {
                page.update(cx, |this, cx| run(this, cx));
            }
        });
    }

    fn send_issue_to_agent(&self, cx: &mut Context<Self>) {
        let Some(issue) = &self.issue_detail else {
            return;
        };
        let dir = std::env::temp_dir().join("agentory").join("issues");
        let path = dir.join(format!("{}-{}.md", self.project_id, issue.number));
        let written = std::fs::create_dir_all(&dir)
            .and_then(|()| std::fs::write(&path, issue_markdown(issue)));
        if let Err(e) = written {
            tracing::error!("write issue file {}: {e}", path.display());
            toast::error(
                cx,
                t_args("github.send_to_agent_failed", &[("error", &e.to_string())]),
            );
            return;
        }
        let prefs = &self.stores.prefs.read(cx).prefs;
        let mut input = prefs.issue_prompt_for(&path.to_string_lossy(), issue.number, &issue.title);
        if prefs.send_issue_prompt {
            input.push('\r');
        }
        sessions::begin_new_input(&self.project_id, Some(input), cx);
        cx.emit(Navigate(Route::Sessions));
    }

    fn ask_merge(&mut self, number: u64, cx: &mut Context<Self>) {
        let options = ConfirmOptions::new(
            t("github.merge.title"),
            t_args("github.merge.body", &[("n", &number.to_string())]),
        )
        .labels(t("github.merge"), t("common.cancel"));
        self.ask(options, cx, move |this, cx| {
            this.op(t("github.merge"), false, cx, move |dir| async move {
                gh::pr_merge(&dir, number).await
            });
        });
    }

    fn ask_close(&mut self, number: u64, cx: &mut Context<Self>) {
        let is_pr = self.tab == Tab::Prs;
        let options = ConfirmOptions::new(
            t_args("github.close.title", &[("n", &number.to_string())]),
            t_args("github.close.body", &[("n", &number.to_string())]),
        )
        .labels(t("github.close"), t("common.cancel"));
        self.ask(options, cx, move |this, cx| {
            this.op(t("github.close"), false, cx, move |dir| async move {
                if is_pr {
                    gh::pr_close(&dir, number).await
                } else {
                    gh::issue_close(&dir, number).await
                }
            });
        });
    }

    fn reopen(&mut self, number: u64, cx: &mut Context<Self>) {
        let is_pr = self.tab == Tab::Prs;
        self.op(t("github.reopen"), false, cx, move |dir| async move {
            if is_pr {
                gh::pr_reopen(&dir, number).await
            } else {
                gh::issue_reopen(&dir, number).await
            }
        });
    }

    fn ask_delete_branch(&mut self, branch: String, cx: &mut Context<Self>) {
        let options = ConfirmOptions::new(
            t("github.delete_branch.title"),
            t_args("github.delete_branch.body", &[("branch", &branch)]),
        )
        .labels(t("github.delete_branch"), t("common.cancel"));
        self.ask(options, cx, move |this, cx| {
            this.op(
                t("github.delete_branch"),
                false,
                cx,
                move |dir| async move { gh::delete_branch(&dir, &branch).await },
            );
        });
    }

    fn checkout(&mut self, number: u64, cx: &mut Context<Self>) {
        self.op(t("github.checkout"), false, cx, move |dir| async move {
            gh::pr_checkout(&dir, number).await
        });
    }

    fn submit_comment(&mut self, cx: &mut Context<Self>) {
        let body = self.comment.read(cx).value().trim().to_string();
        if body.is_empty() {
            toast::error(cx, t("github.comment.empty"));
            return;
        }
        let is_pr = self.tab == Tab::Prs;
        let Some(number) = (if is_pr { self.pr } else { self.issue }) else {
            return;
        };
        self.op(t("github.comment"), true, cx, move |dir| async move {
            if is_pr {
                gh::pr_comment(&dir, number, &body).await
            } else {
                gh::issue_comment(&dir, number, &body).await
            }
        });
    }

    fn open_create(&mut self, cx: &mut Context<Self>) {
        self.creating = true;
        cx.notify();
    }

    fn close_create(&mut self, cx: &mut Context<Self>) {
        if !self.creating {
            return;
        }
        self.creating = false;
        self.new_title.update(cx, |i, cx| i.set_value("", cx));
        self.new_body.update(cx, |b, cx| b.set_value("", cx));
        cx.notify();
    }

    fn create(&mut self, cx: &mut Context<Self>) {
        let title = self.new_title.read(cx).value().trim().to_string();
        if title.is_empty() {
            toast::error(cx, t("github.new.title_required"));
            return;
        }
        let body = self.new_body.read(cx).value().trim().to_string();
        let is_pr = self.tab == Tab::Prs;
        let label = t(if is_pr {
            "github.new.pr"
        } else {
            "github.new.issue"
        });
        self.op(label, true, cx, move |dir| async move {
            if is_pr {
                gh::pr_create(&dir, &title, &body).await
            } else {
                gh::issue_create(&dir, &title, &body).await
            }
        });
    }

    fn header(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .flex()
            .items_center()
            .gap(px(10.0))
            .child(
                div()
                    .flex_1()
                    .text_size(px(20.0))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child(t("github.title")),
            )
            .child(
                div()
                    .id("github-refresh")
                    .flex()
                    .items_center()
                    .justify_center()
                    .size(px(26.0))
                    .rounded(px(6.0))
                    .cursor_pointer()
                    .hover(|s| s.bg(theme::hover()))
                    .on_click(cx.listener(|this, _, _, cx| {
                        if this.setup == Some(Setup::Ready) {
                            this.refresh(cx);
                        } else {
                            this.check_setup(cx);
                        }
                    }))
                    .child(icon(Icon::Refresh, px(15.0)).text_color(theme::text())),
            )
            .into_any_element()
    }

    fn pill(
        id: &'static str,
        label: String,
        active: bool,
        on_click: impl Fn(&gpui::ClickEvent, &mut Window, &mut gpui::App) + 'static,
    ) -> AnyElement {
        div()
            .id(id)
            .px(px(10.0))
            .h(px(24.0))
            .flex()
            .items_center()
            .rounded(px(4.0))
            .cursor_pointer()
            .text_size(px(12.5))
            .when(active, |s| {
                s.bg(theme::accent_soft()).text_color(theme::accent())
            })
            .when(!active, |s| {
                s.text_color(theme::text_muted())
                    .hover(|s| s.bg(theme::hover()))
            })
            .on_click(on_click)
            .child(label)
            .into_any_element()
    }

    fn tabs(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .flex()
            .items_center()
            .gap(px(4.0))
            .child(Self::pill(
                "github-tab-prs",
                t("github.tab.prs"),
                self.tab == Tab::Prs,
                cx.listener(|this, _, _, cx| this.set_tab(Tab::Prs, cx)),
            ))
            .child(Self::pill(
                "github-tab-issues",
                t("github.tab.issues"),
                self.tab == Tab::Issues,
                cx.listener(|this, _, _, cx| this.set_tab(Tab::Issues, cx)),
            ))
            .into_any_element()
    }

    fn failure_banner(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let failure = self.failure.as_ref()?;
        let open = self.failure_open;
        Some(
            div()
                .flex()
                .flex_col()
                .gap(px(6.0))
                .p(px(10.0))
                .rounded(px(6.0))
                .border_1()
                .border_color(theme::error())
                .bg(tint(theme::error()))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(10.0))
                        .child(
                            div()
                                .flex_1()
                                .text_size(px(12.5))
                                .child(t_args("github.failed", &[("op", &failure.op)])),
                        )
                        .child(small_button(
                            "github-output-toggle",
                            t(if open {
                                "git.output_hide"
                            } else {
                                "git.output_show"
                            }),
                            cx.listener(|this, _, _, cx| {
                                this.failure_open = !this.failure_open;
                                cx.notify();
                            }),
                        ))
                        .child(icon_button(
                            "github-failure-close",
                            Icon::Close,
                            cx.listener(|this, _, _, cx| {
                                this.failure = None;
                                cx.notify();
                            }),
                        )),
                )
                .when(open, |s| {
                    s.child(
                        div()
                            .id("github-failure-output")
                            .max_h(px(180.0))
                            .overflow_y_scroll()
                            .font_family(theme::FONT)
                            .text_size(px(11.5))
                            .text_color(theme::text_secondary())
                            .child(failure.output.clone()),
                    )
                })
                .into_any_element(),
        )
    }

    fn list_row(
        &self,
        id: String,
        number: u64,
        title: &str,
        state: &str,
        draft: bool,
        meta: String,
        selected: bool,
        on_click: impl Fn(&gpui::ClickEvent, &mut Window, &mut gpui::App) + 'static,
    ) -> AnyElement {
        div()
            .id(SharedString::from(id))
            .flex()
            .flex_col()
            .flex_none()
            .gap(px(2.0))
            .px(px(8.0))
            .py(px(5.0))
            .rounded(px(4.0))
            .cursor_pointer()
            .when(selected, |s| s.bg(theme::accent_soft()))
            .when(!selected, |s| s.hover(|s| s.bg(theme::hover())))
            .on_click(on_click)
            .child(
                div()
                    .flex()
                    .gap(px(6.0))
                    .text_size(px(12.5))
                    .child(
                        div()
                            .flex_none()
                            .text_color(theme::text_muted())
                            .child(format!("#{number}")),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_color(theme::text())
                            .child(title.to_string()),
                    ),
            )
            .child(
                div()
                    .flex()
                    .gap(px(8.0))
                    .text_size(px(11.0))
                    .text_color(theme::text_muted())
                    .child(
                        div()
                            .flex_none()
                            .text_color(state_color(state))
                            .child(state_label(state, draft)),
                    )
                    .child(div().flex_1().min_w_0().truncate().child(meta)),
            )
            .into_any_element()
    }

    fn left_pane(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut rows: Vec<AnyElement> = Vec::new();
        match self.tab {
            Tab::Prs => {
                for p in &self.prs {
                    let n = p.number;
                    rows.push(self.list_row(
                        format!("github-pr-{n}"),
                        n,
                        &p.title,
                        &p.state,
                        p.is_draft,
                        format!("{} · {}", p.author.login, day(&p.updated_at)),
                        self.pr == Some(n),
                        cx.listener(move |this, _, _, cx| this.select_pr(n, cx)),
                    ));
                }
            }
            Tab::Issues => {
                for i in &self.issues {
                    let n = i.number;
                    rows.push(self.list_row(
                        format!("github-issue-{n}"),
                        n,
                        &i.title,
                        &i.state,
                        false,
                        format!("{} · {}", i.author.login, day(&i.updated_at)),
                        self.issue == Some(n),
                        cx.listener(move |this, _, _, cx| this.select_issue(n, cx)),
                    ));
                }
            }
        }
        let empty = match self.tab {
            Tab::Prs => self.prs.is_empty(),
            Tab::Issues => self.issues.is_empty(),
        };
        if let Some(e) = &self.list_error {
            rows.push(
                muted_note(t_args("github.read_failed", &[("error", e)]).into()).into_any_element(),
            );
        } else if self.list_loading && empty {
            rows.push(muted_note(t("github.loading").into()).into_any_element());
        } else if empty {
            rows.push(
                muted_note(
                    t(match self.tab {
                        Tab::Prs => "github.empty.prs",
                        Tab::Issues => "github.empty.issues",
                    })
                    .into(),
                )
                .into_any_element(),
            );
        }
        div()
            .flex()
            .flex_col()
            .flex_none()
            .w(px(LEFT_PANE_W))
            .min_h_0()
            .gap(px(8.0))
            .child(div().flex().child(small_button(
                "github-new",
                t(match self.tab {
                    Tab::Prs => "github.new.pr",
                    Tab::Issues => "github.new.issue",
                }),
                cx.listener(|this, _, _, cx| this.open_create(cx)),
            )))
            .child(
                self.list_bar.overlay(
                    div()
                        .id("github-list-scroll")
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_h_0()
                        .overflow_y_scroll()
                        .children(rows),
                ),
            )
            .into_any_element()
    }

    fn panel() -> gpui::Div {
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .rounded(px(6.0))
            .border_1()
            .border_color(theme::border_subtle())
            .bg(theme::bg_app())
    }

    fn placeholder(text: String) -> AnyElement {
        Self::panel()
            .items_center()
            .justify_center()
            .child(muted_note(text.into()))
            .into_any_element()
    }

    fn create_form(&self, cx: &mut Context<Self>) -> AnyElement {
        Self::panel()
            .gap(px(10.0))
            .p(px(12.0))
            .child(
                div()
                    .text_size(px(14.0))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child(t(match self.tab {
                        Tab::Prs => "github.new.pr",
                        Tab::Issues => "github.new.issue",
                    })),
            )
            .child(
                div()
                    .flex()
                    .flex_none()
                    .h(px(28.0))
                    .rounded(px(4.0))
                    .border_1()
                    .border_color(theme::border_subtle())
                    .px(px(8.0))
                    .child(self.new_title.clone()),
            )
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .rounded(px(4.0))
                    .border_1()
                    .border_color(theme::border_subtle())
                    .child(self.new_body.clone()),
            )
            .when(self.tab == Tab::Prs, |s| {
                s.child(muted_note(t("github.new.pr_hint").into()))
            })
            .child(
                div()
                    .flex()
                    .gap(px(8.0))
                    .child(button(
                        "github-create",
                        t("github.new.create"),
                        ButtonVariant::Classic,
                        cx.listener(|this, _, _, cx| this.create(cx)),
                    ))
                    .child(button(
                        "github-create-cancel",
                        t("common.cancel"),
                        ButtonVariant::Secondary,
                        cx.listener(|this, _, _, cx| this.close_create(cx)),
                    )),
            )
            .into_any_element()
    }

    fn card(
        id: &'static str,
        index: usize,
        header: String,
        sub: Option<String>,
        body: AnyElement,
    ) -> AnyElement {
        div()
            .id(SharedString::from(format!("{id}-{index}")))
            .flex()
            .flex_col()
            .flex_none()
            .rounded(px(6.0))
            .border_1()
            .border_color(theme::border_subtle())
            .overflow_hidden()
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(px(6.0))
                    .px(px(10.0))
                    .py(px(6.0))
                    .bg(theme::bg_chrome())
                    .border_b_1()
                    .border_color(theme::border_subtle())
                    .text_size(px(11.5))
                    .text_color(theme::text_muted())
                    .child(header)
                    .children(sub),
            )
            .child(div().p(px(10.0)).child(body))
            .into_any_element()
    }

    fn comment_block(c: &Comment, index: usize) -> AnyElement {
        Self::card(
            "github-comment",
            index,
            byline("github.commented", &c.author.login, &c.created_at),
            None,
            div()
                .text_size(px(12.5))
                .text_color(theme::text())
                .child(c.body.clone())
                .into_any_element(),
        )
    }

    fn body_block(
        body: &str,
        author: &str,
        created_at: &str,
        branches: Option<String>,
    ) -> AnyElement {
        let empty = body.trim().is_empty();
        Self::card(
            "github-body",
            0,
            byline("github.opened", author, created_at),
            branches,
            div()
                .text_size(px(12.5))
                .text_color(if empty {
                    theme::text_muted()
                } else {
                    theme::text()
                })
                .child(if empty {
                    t("github.no_description")
                } else {
                    body.to_string()
                })
                .into_any_element(),
        )
    }

    fn comment_box(&self, toggle: Option<AnyElement>, cx: &mut Context<Self>) -> AnyElement {
        div()
            .flex()
            .flex_col()
            .flex_none()
            .gap(px(8.0))
            .child(
                div()
                    .text_size(px(13.0))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child(t("github.add_comment")),
            )
            .child(
                div()
                    .flex()
                    .h(px(88.0))
                    .rounded(px(6.0))
                    .border_1()
                    .border_color(theme::border_subtle())
                    .child(self.comment.clone()),
            )
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap(px(8.0))
                    .children(toggle)
                    .child(button(
                        "github-comment",
                        t("github.comment"),
                        ButtonVariant::Classic,
                        cx.listener(|this, _, _, cx| this.submit_comment(cx)),
                    )),
            )
            .into_any_element()
    }

    fn state_toggle(&self, number: u64, state: &str, cx: &mut Context<Self>) -> Option<AnyElement> {
        match state {
            "OPEN" => Some(
                button(
                    "github-close",
                    t("github.close"),
                    ButtonVariant::Secondary,
                    cx.listener(move |this, _, _, cx| this.ask_close(number, cx)),
                )
                .into_any_element(),
            ),
            "CLOSED" => Some(
                button(
                    "github-reopen",
                    t("github.reopen"),
                    ButtonVariant::Secondary,
                    cx.listener(move |this, _, _, cx| this.reopen(number, cx)),
                )
                .into_any_element(),
            ),
            _ => None,
        }
    }

    fn detail_head(
        &self,
        number: u64,
        title: &str,
        state: &str,
        draft: bool,
        actions: Vec<AnyElement>,
    ) -> AnyElement {
        div()
            .flex()
            .flex_col()
            .flex_none()
            .gap(px(8.0))
            .child(
                div()
                    .flex()
                    .items_start()
                    .gap(px(8.0))
                    .text_size(px(28.0))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child(div().min_w_0().child(title.to_string()))
                    .child(
                        div()
                            .flex_none()
                            .font_weight(gpui::FontWeight::NORMAL)
                            .text_color(theme::text_muted())
                            .child(format!("#{number}")),
                    ),
            )
            .child(
                div().flex().child(
                    div()
                        .flex_none()
                        .px(px(8.0))
                        .rounded(px(8.0))
                        .text_size(px(11.0))
                        .text_color(state_color(state))
                        .bg(tint(state_color(state)))
                        .child(state_label(state, draft)),
                ),
            )
            .when(!actions.is_empty(), |s| {
                s.child(div().flex().flex_wrap().gap(px(8.0)).children(actions))
            })
            .into_any_element()
    }

    fn check_row(c: &Check, index: usize) -> AnyElement {
        let (glyph, color) = match c.result() {
            CheckState::Passed => ("✓", theme::success()),
            CheckState::Failed => ("✗", theme::error()),
            CheckState::Pending => ("…", theme::warning()),
            CheckState::Skipped => ("–", theme::text_muted()),
        };
        let url = c.url().to_string();
        let linked = !url.is_empty();
        div()
            .id(SharedString::from(format!("github-check-{index}")))
            .flex()
            .items_center()
            .gap(px(8.0))
            .flex_none()
            .px(px(6.0))
            .h(px(26.0))
            .rounded(px(4.0))
            .when(linked, |s| {
                s.cursor_pointer()
                    .hover(|s| s.bg(theme::hover()))
                    .on_click(move |_, _, cx| cx.open_url(&url))
            })
            .child(div().w(px(14.0)).flex_none().text_color(color).child(glyph))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(px(12.5))
                    .text_color(theme::text())
                    .child(c.title().to_string()),
            )
            .into_any_element()
    }

    fn pr_detail_pane(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(pr) = &self.pr_detail else {
            return Self::placeholder(match &self.detail_error {
                Some(e) => t_args("github.read_failed", &[("error", e)]),
                None => t("github.loading"),
            });
        };
        let number = pr.number;
        let open = pr.state == "OPEN";
        let mut actions: Vec<AnyElement> = vec![
            small_button(
                "github-checkout",
                t("github.checkout"),
                cx.listener(move |this, _, _, cx| this.checkout(number, cx)),
            )
            .into_any_element(),
        ];
        if open {
            actions.push(
                small_button(
                    "github-merge",
                    t("github.merge"),
                    cx.listener(move |this, _, _, cx| this.ask_merge(number, cx)),
                )
                .into_any_element(),
            );
        } else if pr.state == "MERGED" && self.branch_deletable {
            let branch = pr.head_ref_name.clone();
            actions.push(
                small_button(
                    "github-delete-branch",
                    t("github.delete_branch"),
                    cx.listener(move |this, _, _, cx| this.ask_delete_branch(branch.clone(), cx)),
                )
                .into_any_element(),
            );
        }
        let head = self.detail_head(number, &pr.title, &pr.state, pr.is_draft, actions);
        let views = div()
            .flex()
            .gap(px(4.0))
            .child(Self::pill(
                "github-view-conversation",
                t("github.conversation"),
                self.pr_view == PrView::Conversation,
                cx.listener(|this, _, _, cx| this.set_pr_view(PrView::Conversation, cx)),
            ))
            .child(Self::pill(
                "github-view-files",
                t("github.files"),
                self.pr_view == PrView::Files,
                cx.listener(|this, _, _, cx| this.set_pr_view(PrView::Files, cx)),
            ))
            .child(Self::pill(
                "github-view-checks",
                format!("{} ({})", t("github.checks"), pr.status_check_rollup.len()),
                self.pr_view == PrView::Checks,
                cx.listener(|this, _, _, cx| this.set_pr_view(PrView::Checks, cx)),
            ));
        let content: AnyElement = match self.pr_view {
            PrView::Conversation => {
                let branches = format!("{} → {}", pr.head_ref_name, pr.base_ref_name);
                let mut items: Vec<AnyElement> = vec![Self::body_block(
                    &pr.body,
                    &pr.author.login,
                    &pr.created_at,
                    Some(branches),
                )];
                items.extend(
                    pr.comments
                        .iter()
                        .enumerate()
                        .map(|(i, c)| Self::comment_block(c, i)),
                );
                items.push(self.comment_box(self.state_toggle(number, &pr.state, cx), cx));
                self.scroll_body("github-pr-scroll", items)
            }
            PrView::Files => diff_view(
                window,
                &self.diff,
                self.diff_hidden,
                &self.diff_scroll,
                Some(pr.title.clone()),
                t("git.no_diff"),
            ),
            PrView::Checks => {
                let items: Vec<AnyElement> = if pr.status_check_rollup.is_empty() {
                    vec![muted_note(t("github.no_checks").into()).into_any_element()]
                } else {
                    pr.status_check_rollup
                        .iter()
                        .enumerate()
                        .map(|(i, c)| Self::check_row(c, i))
                        .collect()
                };
                self.scroll_body("github-checks-scroll", items)
            }
        };
        Self::panel()
            .gap(px(10.0))
            .p(px(12.0))
            .child(head)
            .child(views)
            .child(content)
            .into_any_element()
    }

    fn edit_actions(
        id: &'static str,
        save: impl Fn(&mut Self, &mut Context<Self>) + 'static,
        cancel: impl Fn(&mut Self, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .flex()
            .gap(px(8.0))
            .child(button(
                SharedString::from(format!("{id}-save")),
                t("github.save"),
                ButtonVariant::Classic,
                cx.listener(move |this, _, _, cx| save(this, cx)),
            ))
            .child(button(
                SharedString::from(format!("{id}-cancel")),
                t("common.cancel"),
                ButtonVariant::Secondary,
                cx.listener(move |this, _, _, cx| cancel(this, cx)),
            ))
            .into_any_element()
    }

    fn issue_edit_form(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .flex()
            .flex_col()
            .flex_none()
            .gap(px(8.0))
            .child(
                div()
                    .flex()
                    .flex_none()
                    .h(px(28.0))
                    .rounded(px(4.0))
                    .border_1()
                    .border_color(theme::border_subtle())
                    .px(px(8.0))
                    .child(self.edit_title.clone()),
            )
            .child(
                div()
                    .flex()
                    .h(px(160.0))
                    .rounded(px(4.0))
                    .border_1()
                    .border_color(theme::border_subtle())
                    .child(self.edit_body.clone()),
            )
            .child(Self::edit_actions(
                "github-issue-edit",
                Self::save_issue_edit,
                Self::cancel_issue_edit,
                cx,
            ))
            .into_any_element()
    }

    fn issue_comment_block(&self, c: &Comment, index: usize, cx: &mut Context<Self>) -> AnyElement {
        let id = c.id();
        let own = id.is_some() && self.me.as_deref() == Some(c.author.login.as_str());
        let header = byline("github.commented", &c.author.login, &c.created_at);
        let body = match id {
            Some(id) if own && self.editing_comment == Some(id) => div()
                .flex()
                .flex_col()
                .gap(px(8.0))
                .child(
                    div()
                        .flex()
                        .h(px(120.0))
                        .rounded(px(4.0))
                        .border_1()
                        .border_color(theme::border_subtle())
                        .child(self.edit_comment.clone()),
                )
                .child(Self::edit_actions(
                    "github-comment-edit",
                    Self::save_comment_edit,
                    Self::cancel_comment_edit,
                    cx,
                ))
                .into_any_element(),
            Some(id) if own => {
                let text = c.body.clone();
                div()
                    .flex()
                    .flex_col()
                    .items_start()
                    .gap(px(8.0))
                    .text_size(px(12.5))
                    .text_color(theme::text())
                    .child(c.body.clone())
                    .child(small_button(
                        SharedString::from(format!("github-comment-edit-{index}")),
                        t("github.edit"),
                        cx.listener(move |this, _, _, cx| this.start_comment_edit(id, &text, cx)),
                    ))
                    .into_any_element()
            }
            _ => div()
                .text_size(px(12.5))
                .text_color(theme::text())
                .child(c.body.clone())
                .into_any_element(),
        };
        Self::card("github-comment", index, header, None, body)
    }

    fn picker_panel(&self, issue: &IssueDetail, cx: &mut Context<Self>) -> Option<AnyElement> {
        let picker = self.picker?;
        let (options, selected, id): (&[String], Vec<&str>, &'static str) = match picker {
            Picker::Labels => (
                &self.repo_labels,
                issue.labels.iter().map(|l| l.name.as_str()).collect(),
                "github-label",
            ),
            Picker::Assignees => (
                &self.repo_users,
                issue.assignees.iter().map(|a| a.login.as_str()).collect(),
                "github-assignee",
            ),
        };
        let chips =
            options.iter().enumerate().map(|(i, name)| {
                let on = selected.contains(&name.as_str());
                let pick = name.clone();
                div()
                    .id(SharedString::from(format!("{id}-{i}")))
                    .px(px(8.0))
                    .py(px(2.0))
                    .rounded(px(10.0))
                    .border_1()
                    .border_color(if on {
                        theme::accent()
                    } else {
                        theme::border_subtle()
                    })
                    .text_size(px(11.5))
                    .text_color(if on {
                        theme::accent()
                    } else {
                        theme::text_secondary()
                    })
                    .cursor_pointer()
                    .hover(|s| s.bg(theme::hover()))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.toggle_pick(picker, pick.clone(), on, cx)
                    }))
                    .child(name.clone())
            });
        Some(
            div()
                .flex()
                .flex_wrap()
                .flex_none()
                .gap(px(6.0))
                .when(options.is_empty(), |s| {
                    s.child(muted_note(t("github.loading").into()))
                })
                .children(chips)
                .into_any_element(),
        )
    }

    fn scroll_body(&self, id: &'static str, items: Vec<AnyElement>) -> AnyElement {
        self.detail_bar
            .overlay(
                div()
                    .id(id)
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .gap(px(10.0))
                    .overflow_y_scroll()
                    .children(items),
            )
            .flex_1()
            .min_h_0()
            .into_any_element()
    }

    fn issue_detail_pane(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(issue) = &self.issue_detail else {
            return Self::placeholder(match &self.detail_error {
                Some(e) => t_args("github.read_failed", &[("error", e)]),
                None => t("github.loading"),
            });
        };
        let number = issue.number;
        let actions = vec![
            small_button(
                "github-send-agent",
                t("github.send_to_agent"),
                cx.listener(|this, _, _, cx| this.send_issue_to_agent(cx)),
            )
            .into_any_element(),
            small_button(
                "github-edit-issue",
                t("github.edit"),
                cx.listener(|this, _, _, cx| this.start_issue_edit(cx)),
            )
            .into_any_element(),
            small_button(
                "github-pick-labels",
                t("github.labels"),
                cx.listener(|this, _, _, cx| this.toggle_picker(Picker::Labels, cx)),
            )
            .into_any_element(),
            small_button(
                "github-pick-assignees",
                t("github.assignees"),
                cx.listener(|this, _, _, cx| this.toggle_picker(Picker::Assignees, cx)),
            )
            .into_any_element(),
        ];
        let head = self.detail_head(number, &issue.title, &issue.state, false, actions);
        let mut items: Vec<AnyElement> = Vec::new();
        items.extend(self.picker_panel(issue, cx));
        items.push(if self.editing_issue {
            self.issue_edit_form(cx)
        } else {
            Self::body_block(&issue.body, &issue.author.login, &issue.created_at, None)
        });
        items.extend(
            issue
                .comments
                .iter()
                .enumerate()
                .map(|(i, c)| self.issue_comment_block(c, i, cx)),
        );
        if self.editing_issue || self.editing_comment.is_some() {
            items.push(muted_note(t("github.editing_locked").into()).into_any_element());
        } else {
            items.push(self.comment_box(self.state_toggle(number, &issue.state, cx), cx));
        }
        Self::panel()
            .gap(px(10.0))
            .p(px(12.0))
            .child(head)
            .child(self.scroll_body("github-issue-scroll", items))
            .into_any_element()
    }

    fn right_pane(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        if self.creating {
            return self.create_form(cx);
        }
        match self.tab {
            Tab::Prs if self.pr.is_some() => self.pr_detail_pane(window, cx),
            Tab::Issues if self.issue.is_some() => self.issue_detail_pane(cx),
            Tab::Prs => Self::placeholder(t("github.select.pr")),
            Tab::Issues => Self::placeholder(t("github.select.issue")),
        }
    }

    fn guide(&self, setup: &Setup) -> AnyElement {
        let key = match setup {
            Setup::NotInstalled => "github.guide.not_installed",
            Setup::NotAuthenticated => "github.guide.not_authenticated",
            _ => "github.guide.no_remote",
        };
        muted_note(t(key).into()).into_any_element()
    }
}

impl Render for GithubPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let ready = self.setup == Some(Setup::Ready);
        let body: AnyElement = match &self.setup {
            None => muted_note(t("github.loading").into()).into_any_element(),
            Some(Setup::Ready) => div()
                .flex()
                .flex_1()
                .min_h_0()
                .gap(px(12.0))
                .child(self.left_pane(cx))
                .child(self.right_pane(window, cx))
                .into_any_element(),
            Some(other) => self.guide(other),
        };
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .p(px(24.0))
            .gap(px(12.0))
            .child(self.header(cx))
            .when(ready, |s| s.child(self.tabs(cx)))
            .children(self.failure_banner(cx))
            .child(body)
    }
}

fn issue_markdown(issue: &IssueDetail) -> String {
    let mut out = format!(
        "# {} (#{})\n\nState: {}\nAuthor: {}\n\n## Description\n\n",
        issue.title, issue.number, issue.state, issue.author.login
    );
    if issue.body.trim().is_empty() {
        out.push_str(&t("github.issue.no_description"));
    } else {
        out.push_str(issue.body.trim());
    }
    out.push('\n');
    for c in &issue.comments {
        out.push_str(&format!(
            "\n## Comment by {} ({})\n\n{}\n",
            c.author.login,
            c.created_at,
            c.body.trim()
        ));
    }
    out
}

#[cfg(test)]
mod issue_tests {
    use super::*;

    fn issue(body: &str) -> IssueDetail {
        serde_json::from_value(serde_json::json!({
            "number": 5, "title": "Crash", "state": "OPEN", "body": body,
            "author": {"login": "ann"},
            "comments": [{"author": {"login": "bob"}, "body": "me too", "createdAt": "2026-01-01"}]
        }))
        .unwrap()
    }

    #[test]
    fn markdown_has_header_body_and_comments() {
        let md = issue_markdown(&issue("It breaks"));
        assert!(md.starts_with("# Crash (#5)"));
        assert!(md.contains("Author: ann"));
        assert!(md.contains("It breaks"));
        assert!(md.contains("## Comment by bob"));
        assert!(md.contains("me too"));
    }

    #[test]
    fn empty_body_is_noted() {
        assert!(issue_markdown(&issue("  ")).contains("## Description\n\n"));
    }
}
