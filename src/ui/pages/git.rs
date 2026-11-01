//! Git page: a VS Code style Source Control view of the project's repo. Changed files in three
//! groups (staged, changes, conflicts) on the left with a commit box, the unified diff of the
//! selected file on the right, and the branch with sync in the header.
//!
//! State comes from `git status`, polled while the page is open: git itself skips what
//! `.gitignore` ignores, so build directories do not cause refreshes.

use std::future::Future;
use std::path::PathBuf;
use std::time::Duration;

use gpui::{
    AnyElement, Context, Entity, EventEmitter, InteractiveElement, IntoElement, ParentElement,
    Point, Render, Rgba, ScrollHandle, SharedString, StatefulInteractiveElement, Styled,
    Subscription, Window, div, prelude::*, px,
};

use super::skills::{icon_button, muted_note};
use crate::backend::git::{self, Change, ChangeKind, DiffLine, LineKind, Status};
use crate::i18n::{t, t_args, t_n};
use crate::route::{GoBack, Navigate};
use crate::runtime;
use crate::state::Stores;
use crate::ui::components::button::{ButtonVariant, button, small_button};
use crate::ui::components::text_area::{TextArea, TextAreaEvent};
use crate::ui::confirm::{self, ConfirmOptions};
use crate::ui::icons::{Icon, icon};
use crate::ui::{theme, toast};

/// How often the status is re-read while the page is open.
const POLL: Duration = Duration::from_secs(3);
/// Diff lines drawn; a bigger diff is cut with a note.
const MAX_DIFF_LINES: usize = 3000;
const LEFT_PANE_W: f32 = 340.0;

enum Repo {
    Loading,
    Missing,
    Ready,
    Failed(String),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Group {
    Staged,
    Changes,
    Conflicts,
}

impl Group {
    fn key(self) -> &'static str {
        match self {
            Self::Staged => "staged",
            Self::Changes => "changes",
            Self::Conflicts => "conflicts",
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
struct Pick {
    path: String,
    group: Group,
}

/// The output of the last failed git command.
struct Failure {
    op: String,
    output: String,
}

pub struct GitPage {
    stores: Stores,
    project_id: String,
    repo: Repo,
    root: Option<PathBuf>,
    status: Status,
    pick: Option<Pick>,
    diff: Vec<DiffLine>,
    diff_hidden: usize,
    /// Bumped per diff request so a slow, stale answer is dropped.
    diff_gen: u64,
    diff_scroll: ScrollHandle,
    message: Entity<TextArea>,
    /// A local command (stage, commit, discard…) is running.
    busy: bool,
    /// A fetch or sync is running.
    net_busy: bool,
    refreshing: bool,
    /// A refresh was asked for while one was running: run another when it ends.
    refresh_again: bool,
    failure: Option<Failure>,
    failure_open: bool,
    _subs: Vec<Subscription>,
}

impl EventEmitter<Navigate> for GitPage {}
impl EventEmitter<GoBack> for GitPage {}

type Loaded = Option<(PathBuf, Status)>;

async fn load(path: PathBuf) -> Result<Loaded, String> {
    let Some(root) = git::repo_root(&path).await? else {
        return Ok(None);
    };
    let status = git::status(&root).await?;
    Ok(Some((root, status)))
}

fn kind_color(kind: ChangeKind) -> Rgba {
    match kind {
        ChangeKind::Modified => theme::warning(),
        ChangeKind::Added | ChangeKind::Untracked => theme::success(),
        ChangeKind::Deleted | ChangeKind::Conflicted => theme::error(),
        ChangeKind::Renamed => theme::accent(),
    }
}

fn tint(color: Rgba) -> Rgba {
    Rgba {
        alpha: 0.14,
        ..color
    }
}

impl GitPage {
    pub fn new(stores: Stores, project_id: String, cx: &mut Context<Self>) -> Self {
        let message = cx.new(TextArea::new);
        let subs = vec![cx.subscribe(&message, |this, _, ev: &TextAreaEvent, cx| {
            if matches!(ev, TextAreaEvent::Submit) {
                this.commit(cx);
            }
        })];
        let mut this = Self {
            stores,
            project_id,
            repo: Repo::Loading,
            root: None,
            status: Status::default(),
            pick: None,
            diff: Vec::new(),
            diff_hidden: 0,
            diff_gen: 0,
            diff_scroll: ScrollHandle::new(),
            message,
            busy: false,
            net_busy: false,
            refreshing: false,
            refresh_again: false,
            failure: None,
            failure_open: false,
            _subs: subs,
        };
        this.refresh(cx);
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(POLL).await;
                let alive = this.update(cx, |this, cx| {
                    if !this.busy && !this.net_busy {
                        this.refresh(cx);
                    }
                });
                if alive.is_err() {
                    break;
                }
            }
        })
        .detach();
        this
    }

    // ---- loading -----------------------------------------------------------------------------

    fn refresh(&mut self, cx: &mut Context<Self>) {
        if self.refreshing {
            self.refresh_again = true;
            return;
        }
        let Some(path) = self.stores.projects.read(cx).path_of(&self.project_id) else {
            return;
        };
        self.refreshing = true;
        cx.spawn(async move |this, cx| {
            let result = runtime::run(load(path)).await;
            this.update(cx, |this, cx| {
                this.refreshing = false;
                this.apply(result, cx);
                if std::mem::take(&mut this.refresh_again) {
                    this.refresh(cx);
                }
            })
            .ok();
        })
        .detach();
    }

    fn apply(&mut self, result: Result<Loaded, String>, cx: &mut Context<Self>) {
        match result {
            Ok(None) => {
                let changed = !matches!(self.repo, Repo::Missing);
                self.repo = Repo::Missing;
                self.root = None;
                self.status = Status::default();
                self.pick = None;
                self.diff.clear();
                self.set_badge(0, cx);
                if changed {
                    cx.notify();
                }
            }
            Ok(Some((root, status))) => {
                let first = !matches!(self.repo, Repo::Ready);
                self.repo = Repo::Ready;
                self.root = Some(root);
                let changed = first || status != self.status;
                if changed {
                    self.set_badge(status.total(), cx);
                    self.status = status;
                    self.follow_pick();
                    cx.notify();
                }
                // The same status can hide new edits to the selected file.
                self.load_diff(cx);
            }
            Err(e) => {
                tracing::warn!("git status failed: {e}");
                // A passing hiccup (index lock) must not replace a page that was working.
                if matches!(self.repo, Repo::Loading | Repo::Failed(_)) {
                    self.repo = Repo::Failed(e);
                    cx.notify();
                }
            }
        }
    }

    fn set_badge(&self, count: usize, cx: &mut Context<Self>) {
        let id = self.project_id.clone();
        self.stores
            .git
            .update(cx, |g, cx| g.set_count(&id, count, cx));
    }

    fn list(&self, group: Group) -> &[Change] {
        match group {
            Group::Staged => &self.status.staged,
            Group::Changes => &self.status.unstaged,
            Group::Conflicts => &self.status.conflicts,
        }
    }

    fn change_of(&self, pick: &Pick) -> Option<&Change> {
        self.list(pick.group).iter().find(|c| c.path == pick.path)
    }

    /// Keep the selection on its file after the lists changed: the file may have moved to another
    /// group (just staged), or be gone (just committed).
    fn follow_pick(&mut self) {
        let Some(pick) = self.pick.clone() else {
            return;
        };
        if self.change_of(&pick).is_some() {
            return;
        }
        let moved = [Group::Staged, Group::Changes, Group::Conflicts]
            .into_iter()
            .find(|g| self.list(*g).iter().any(|c| c.path == pick.path));
        match moved {
            Some(group) => {
                self.pick = Some(Pick {
                    path: pick.path,
                    group,
                })
            }
            None => {
                self.pick = None;
                self.diff.clear();
                self.diff_hidden = 0;
            }
        }
    }

    fn select(&mut self, pick: Pick, cx: &mut Context<Self>) {
        if self.pick.as_ref() == Some(&pick) {
            return;
        }
        self.pick = Some(pick);
        self.diff.clear();
        self.diff_hidden = 0;
        self.diff_scroll.set_offset(Point::default());
        self.load_diff(cx);
        cx.notify();
    }

    fn load_diff(&mut self, cx: &mut Context<Self>) {
        let (Some(root), Some(pick)) = (self.root.clone(), self.pick.clone()) else {
            return;
        };
        let Some(change) = self.change_of(&pick).cloned() else {
            return;
        };
        self.diff_gen += 1;
        let generation = self.diff_gen;
        let staged = pick.group == Group::Staged;
        cx.spawn(async move |this, cx| {
            let text =
                runtime::run(async move { git::file_diff(&root, &change, staged).await }).await;
            this.update(cx, |this, cx| {
                if this.diff_gen != generation {
                    return;
                }
                let mut lines = match text {
                    Ok(text) => git::parse_diff(&text),
                    Err(e) => vec![DiffLine {
                        kind: LineKind::Note,
                        old: None,
                        new: None,
                        text: e,
                    }],
                };
                let hidden = lines.len().saturating_sub(MAX_DIFF_LINES);
                lines.truncate(MAX_DIFF_LINES);
                if lines != this.diff || hidden != this.diff_hidden {
                    this.diff = lines;
                    this.diff_hidden = hidden;
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    // ---- commands ----------------------------------------------------------------------------

    /// Run a git command off the UI thread, then refresh. A failure becomes a toast plus the
    /// expandable output under the header.
    fn op<F, Fut>(
        &mut self,
        label: String,
        net: bool,
        clear_message: bool,
        cx: &mut Context<Self>,
        f: F,
    ) where
        F: FnOnce(PathBuf) -> Fut,
        Fut: Future<Output = Result<(), String>> + Send + 'static,
    {
        let Some(root) = self.root.clone() else {
            return;
        };
        if if net { self.net_busy } else { self.busy } {
            return;
        }
        if net {
            self.net_busy = true;
        } else {
            self.busy = true;
        }
        self.failure = None;
        cx.notify();
        let fut = f(root);
        cx.spawn(async move |this, cx| {
            let result = runtime::run(fut).await;
            this.update(cx, |this, cx| {
                if net {
                    this.net_busy = false;
                } else {
                    this.busy = false;
                }
                match result {
                    Ok(()) => {
                        if clear_message {
                            this.message.update(cx, |m, cx| m.set_value("", cx));
                        }
                    }
                    Err(output) => {
                        toast::error(cx, t_args("git.failed", &[("op", &label)]));
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

    fn paths_of(change: &Change) -> Vec<String> {
        let mut paths = vec![change.path.clone()];
        paths.extend(change.from.clone());
        paths
    }

    fn stage(&mut self, paths: Vec<String>, cx: &mut Context<Self>) {
        self.op(t("git.stage"), false, false, cx, move |root| async move {
            git::stage(&root, &paths).await
        });
    }

    fn stage_all(&mut self, cx: &mut Context<Self>) {
        self.op(t("git.stage_all"), false, false, cx, |root| async move {
            git::stage_everything(&root).await
        });
    }

    fn unstage(&mut self, paths: Vec<String>, cx: &mut Context<Self>) {
        self.op(t("git.unstage"), false, false, cx, move |root| async move {
            git::unstage(&root, &paths).await
        });
    }

    fn unstage_all(&mut self, cx: &mut Context<Self>) {
        self.op(t("git.unstage_all"), false, false, cx, |root| async move {
            git::unstage_everything(&root).await
        });
    }

    fn ask_discard(&mut self, change: Change, cx: &mut Context<Self>) {
        let key = if change.kind == ChangeKind::Untracked {
            "git.discard.body_untracked"
        } else {
            "git.discard.body"
        };
        let options = ConfirmOptions::new(
            t("git.discard.title"),
            t_args(key, &[("path", &change.path)]),
        )
        .labels(t("git.discard"), t("common.cancel"));
        let page = cx.entity();
        confirm::ask(cx, options, move |answer, _, cx| {
            if answer == Some(true) {
                page.update(cx, |this, cx| {
                    this.op(t("git.discard"), false, false, cx, move |root| async move {
                        git::discard(&root, &change).await
                    });
                });
            }
        });
    }

    fn ask_discard_all(&mut self, cx: &mut Context<Self>) {
        let count = self.status.unstaged.len();
        let options =
            ConfirmOptions::new(t("git.discard.title"), t_n("git.discard_all.body", count))
                .labels(t("git.discard_all"), t("common.cancel"));
        let page = cx.entity();
        confirm::ask(cx, options, move |answer, _, cx| {
            if answer == Some(true) {
                page.update(cx, |this, cx| {
                    this.op(t("git.discard_all"), false, false, cx, |root| async move {
                        git::discard_everything(&root).await
                    });
                });
            }
        });
    }

    fn commit(&mut self, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let message = self.message.read(cx).value().trim().to_string();
        if message.is_empty() {
            toast::error(cx, t("git.commit.empty"));
            return;
        }
        if !self.status.staged.is_empty() {
            self.run_commit(message, false, cx);
            return;
        }
        if self.status.unstaged.is_empty() {
            toast::error(cx, t("git.commit.nothing"));
            return;
        }
        // Like VS Code: with nothing staged, offer to commit everything.
        let options = ConfirmOptions::new(
            t("git.commit.stage_all.title"),
            t("git.commit.stage_all.body"),
        )
        .labels(t("git.commit.stage_all.confirm"), t("common.cancel"));
        let page = cx.entity();
        confirm::ask(cx, options, move |answer, _, cx| {
            if answer == Some(true) {
                page.update(cx, |this, cx| this.run_commit(message, true, cx));
            }
        });
    }

    fn run_commit(&mut self, message: String, stage_all: bool, cx: &mut Context<Self>) {
        self.op(t("git.commit"), false, true, cx, move |root| async move {
            if stage_all {
                git::stage_everything(&root).await?;
            }
            git::commit(&root, &message).await
        });
    }

    fn fetch(&mut self, cx: &mut Context<Self>) {
        self.op(t("git.fetch"), true, false, cx, |root| async move {
            git::fetch(&root).await
        });
    }

    fn sync(&mut self, cx: &mut Context<Self>) {
        let status = self.status.clone();
        self.op(t("git.sync"), true, false, cx, move |root| async move {
            git::sync(&root, &status).await
        });
    }

    fn init_repo(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.stores.projects.read(cx).path_of(&self.project_id) else {
            return;
        };
        if self.busy {
            return;
        }
        self.busy = true;
        self.failure = None;
        cx.spawn(async move |this, cx| {
            let result = runtime::run(async move { git::init(&path).await }).await;
            this.update(cx, |this, cx| {
                this.busy = false;
                if let Err(output) = result {
                    let op = t("git.init");
                    toast::error(cx, t_args("git.failed", &[("op", &op)]));
                    this.failure = Some(Failure { op, output });
                }
                this.refresh(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    // ---- pieces ------------------------------------------------------------------------------

    fn header(&self, cx: &mut Context<Self>) -> AnyElement {
        let status = &self.status;
        let ready = matches!(self.repo, Repo::Ready);
        let branch = status.branch.clone().unwrap_or_else(|| t("git.detached"));
        let sync_label = if self.net_busy {
            "…".to_string()
        } else if status.has_upstream {
            format!("{}  ↓{} ↑{}", t("git.sync"), status.behind, status.ahead)
        } else {
            t("git.sync_publish")
        };
        div()
            .flex()
            .items_center()
            .gap(px(10.0))
            .child(
                div()
                    .flex_1()
                    .text_size(px(20.0))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child(t("git.title")),
            )
            .when(ready, |s| {
                s.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .px(px(8.0))
                        .h(px(26.0))
                        .rounded(px(4.0))
                        .bg(theme::hover())
                        .child(icon(Icon::Branch, px(13.0)).text_color(theme::text_muted()))
                        .child(div().text_size(px(12.0)).child(branch)),
                )
                .child(small_button(
                    "git-sync",
                    sync_label,
                    cx.listener(|this, _, _, cx| this.sync(cx)),
                ))
                .child(small_button(
                    "git-fetch",
                    t("git.fetch"),
                    cx.listener(|this, _, _, cx| this.fetch(cx)),
                ))
            })
            .child(
                div()
                    .id("git-refresh")
                    .flex()
                    .items_center()
                    .justify_center()
                    .size(px(26.0))
                    .rounded(px(6.0))
                    .cursor_pointer()
                    .hover(|s| s.bg(theme::hover()))
                    .on_click(cx.listener(|this, _, _, cx| this.refresh(cx)))
                    .child(icon(Icon::Refresh, px(15.0)).text_color(theme::text())),
            )
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
                                .child(t_args("git.failed", &[("op", &failure.op)])),
                        )
                        .child(small_button(
                            "git-output-toggle",
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
                            "git-failure-close",
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
                            .id("git-failure-output")
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

    fn commit_box(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .child(
                div()
                    .flex()
                    .h(px(88.0))
                    .rounded(px(6.0))
                    .border_1()
                    .border_color(theme::border_subtle())
                    .bg(theme::bg_app())
                    .child(self.message.clone()),
            )
            .child(
                button(
                    "git-commit",
                    t("git.commit"),
                    ButtonVariant::Classic,
                    cx.listener(|this, _, _, cx| this.commit(cx)),
                )
                .w_full(),
            )
            .into_any_element()
    }

    fn group_header(
        &self,
        group: Group,
        title: String,
        count: usize,
        actions: Vec<AnyElement>,
    ) -> AnyElement {
        let _ = group;
        div()
            .flex()
            .items_center()
            .gap(px(6.0))
            .pt(px(6.0))
            .child(
                div()
                    .flex_1()
                    .text_size(px(11.0))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(theme::text_muted())
                    .child(title.to_uppercase()),
            )
            .children(actions)
            .child(
                div()
                    .px(px(6.0))
                    .rounded(px(8.0))
                    .bg(theme::hover())
                    .text_size(px(11.0))
                    .text_color(theme::text_muted())
                    .child(count.to_string()),
            )
            .into_any_element()
    }

    fn file_row(&self, group: Group, change: &Change, cx: &mut Context<Self>) -> AnyElement {
        let pick = Pick {
            path: change.path.clone(),
            group,
        };
        let selected = self.pick.as_ref() == Some(&pick);
        let (dir, name) = match change.path.rsplit_once('/') {
            Some((d, n)) => (d.to_string(), n.to_string()),
            None => (String::new(), change.path.clone()),
        };
        let id =
            |what: &str| SharedString::from(format!("git-{what}-{}-{}", group.key(), change.path));
        let mut actions: Vec<AnyElement> = Vec::new();
        match group {
            Group::Staged => {
                let paths = Self::paths_of(change);
                actions.push(
                    icon_button(
                        id("unstage"),
                        Icon::Minus,
                        cx.listener(move |this, _, _, cx| this.unstage(paths.clone(), cx)),
                    )
                    .into_any_element(),
                );
            }
            Group::Changes => {
                let for_discard = change.clone();
                let paths = Self::paths_of(change);
                actions.push(
                    icon_button(
                        id("discard"),
                        Icon::Undo,
                        cx.listener(move |this, _, _, cx| {
                            this.ask_discard(for_discard.clone(), cx)
                        }),
                    )
                    .into_any_element(),
                );
                actions.push(
                    icon_button(
                        id("stage"),
                        Icon::Plus,
                        cx.listener(move |this, _, _, cx| this.stage(paths.clone(), cx)),
                    )
                    .into_any_element(),
                );
            }
            Group::Conflicts => {
                let paths = Self::paths_of(change);
                actions.push(
                    icon_button(
                        id("resolve"),
                        Icon::Plus,
                        cx.listener(move |this, _, _, cx| this.stage(paths.clone(), cx)),
                    )
                    .into_any_element(),
                );
            }
        }
        div()
            .flex()
            .items_center()
            .gap(px(4.0))
            .px(px(6.0))
            .h(px(28.0))
            .rounded(px(4.0))
            .when(selected, |s| s.bg(theme::accent_soft()))
            .child(
                div()
                    .id(id("row"))
                    .flex()
                    .flex_1()
                    .min_w_0()
                    .items_center()
                    .gap(px(8.0))
                    .h_full()
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| this.select(pick.clone(), cx)))
                    .child(
                        div()
                            .w(px(12.0))
                            .flex_none()
                            .text_size(px(11.0))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(kind_color(change.kind))
                            .child(change.kind.letter()),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_size(px(12.5))
                            .text_color(theme::text())
                            .child(name),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(px(11.0))
                            .text_color(theme::text_muted())
                            .child(dir),
                    ),
            )
            .children(actions)
            .into_any_element()
    }

    fn left_pane(&self, cx: &mut Context<Self>) -> AnyElement {
        let status = &self.status;
        let mut groups: Vec<AnyElement> = Vec::new();

        if !status.conflicts.is_empty() {
            groups.push(self.group_header(
                Group::Conflicts,
                t("git.group.conflicts"),
                status.conflicts.len(),
                Vec::new(),
            ));
            for c in &status.conflicts {
                groups.push(self.file_row(Group::Conflicts, c, cx));
            }
        }
        if !status.staged.is_empty() {
            let unstage_all = icon_button(
                "git-unstage-all",
                Icon::Minus,
                cx.listener(|this, _, _, cx| this.unstage_all(cx)),
            )
            .into_any_element();
            groups.push(self.group_header(
                Group::Staged,
                t("git.group.staged"),
                status.staged.len(),
                vec![unstage_all],
            ));
            for c in &status.staged {
                groups.push(self.file_row(Group::Staged, c, cx));
            }
        }
        if !status.unstaged.is_empty() {
            let discard_all = icon_button(
                "git-discard-all",
                Icon::Undo,
                cx.listener(|this, _, _, cx| this.ask_discard_all(cx)),
            )
            .into_any_element();
            let stage_all = icon_button(
                "git-stage-all",
                Icon::Plus,
                cx.listener(|this, _, _, cx| this.stage_all(cx)),
            )
            .into_any_element();
            groups.push(self.group_header(
                Group::Changes,
                t("git.group.changes"),
                status.unstaged.len(),
                vec![discard_all, stage_all],
            ));
            for c in &status.unstaged {
                groups.push(self.file_row(Group::Changes, c, cx));
            }
        }
        if status.total() == 0 {
            groups.push(muted_note(t("git.clean").into()).into_any_element());
        }

        div()
            .flex()
            .flex_col()
            .flex_none()
            .w(px(LEFT_PANE_W))
            .min_h_0()
            .gap(px(8.0))
            .child(self.commit_box(cx))
            .child(
                div()
                    .id("git-changes-scroll")
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .children(groups),
            )
            .into_any_element()
    }

    fn diff_pane(&self) -> AnyElement {
        let pane = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .rounded(px(6.0))
            .border_1()
            .border_color(theme::border_subtle())
            .bg(theme::bg_app());
        let Some(pick) = &self.pick else {
            return pane
                .items_center()
                .justify_center()
                .child(muted_note(t("git.select_file").into()))
                .into_any_element();
        };
        let rows: Vec<AnyElement> = self.diff.iter().map(diff_row).collect();
        pane.child(
            div()
                .px(px(12.0))
                .py(px(8.0))
                .border_b_1()
                .border_color(theme::border_subtle())
                .text_size(px(12.5))
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .truncate()
                .child(pick.path.clone()),
        )
        .child(
            div()
                .id("git-diff-scroll")
                .flex()
                .flex_col()
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .track_scroll(&self.diff_scroll)
                .py(px(6.0))
                .when(rows.is_empty(), |s| {
                    s.child(muted_note(t("git.no_diff").into()))
                })
                .children(rows)
                .when(self.diff_hidden > 0, |s| {
                    s.child(muted_note(
                        t_args("git.diff_cut", &[("n", &self.diff_hidden.to_string())]).into(),
                    ))
                }),
        )
        .into_any_element()
    }
}

/// One diff line: old and new line numbers, then the text, tinted by kind.
fn diff_row(line: &DiffLine) -> AnyElement {
    let num = |n: Option<u32>| {
        div()
            .flex_none()
            .w(px(44.0))
            .pr(px(8.0))
            .flex()
            .justify_end()
            .text_color(theme::text_faint())
            .child(n.map(|n| n.to_string()).unwrap_or_default())
    };
    let (bg, color, sign) = match line.kind {
        LineKind::Add => (Some(tint(theme::success())), theme::text(), "+"),
        LineKind::Del => (Some(tint(theme::error())), theme::text(), "-"),
        LineKind::Hunk => (Some(theme::accent_soft()), theme::accent(), ""),
        LineKind::Note => (None, theme::text_muted(), ""),
        LineKind::Context => (None, theme::text_secondary(), " "),
    };
    let text = if line.text.is_empty() {
        " ".to_string()
    } else {
        line.text.clone()
    };
    div()
        .flex()
        .flex_none()
        .min_h(px(18.0))
        .font_family(theme::FONT)
        .text_size(px(12.0))
        .when_some(bg, |s, bg| s.bg(bg))
        .child(num(line.old))
        .child(num(line.new))
        .child(div().flex_none().w(px(14.0)).text_color(color).child(sign))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .pr(px(8.0))
                .text_color(color)
                .child(text),
        )
        .into_any_element()
}

impl Render for GitPage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let body: AnyElement = match &self.repo {
            Repo::Loading => muted_note(t("git.loading").into()).into_any_element(),
            Repo::Failed(e) => {
                muted_note(t_args("git.read_failed", &[("error", e)]).into()).into_any_element()
            }
            Repo::Missing => div()
                .flex()
                .flex_col()
                .items_start()
                .gap(px(12.0))
                .child(muted_note(t("git.not_repo").into()))
                .child(button(
                    "git-init",
                    t("git.init"),
                    ButtonVariant::Classic,
                    cx.listener(|this, _, _, cx| this.init_repo(cx)),
                ))
                .into_any_element(),
            Repo::Ready => div()
                .flex()
                .flex_1()
                .min_h_0()
                .gap(px(12.0))
                .child(self.left_pane(cx))
                .child(self.diff_pane())
                .into_any_element(),
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
            .children(self.failure_banner(cx))
            .child(body)
    }
}
