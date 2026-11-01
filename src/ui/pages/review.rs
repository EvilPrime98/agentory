//! Review page of a "Modify Agentory" session: the diff against `user`, flags for changes that
//! could weaken the gate, the apply status and the actions (apply, discard, restart to install).

use gpui::{
    AnyElement, Context, EventEmitter, InteractiveElement, IntoElement, ParentElement, Render,
    Styled, Window, div, prelude::*, px,
};

use crate::backend::git;
use crate::i18n::{t, t_args};
use crate::route::{GoBack, Navigate, Route};
use crate::runtime;
use crate::selfbuild::clone::USER_BRANCH;
use crate::selfbuild::review::{self, FileChange, Flag};
use crate::selfbuild::run::Stage;
use crate::state::Stores;
use crate::state::review::{Failure, Run, RunPhase, STAGES, StageState};
use crate::ui::components::button::{ButtonVariant, button, small_button};
use crate::ui::confirm::{self, ConfirmOptions};
use crate::ui::theme;

/// Diff lines drawn; a bigger diff is cut with a note (the flags still cover all of it).
const MAX_DIFF_LINES: usize = 3000;
/// Log lines drawn in the log view.
const MAX_LOG_SHOWN: usize = 400;

struct Loaded {
    files: Vec<FileChange>,
    flags: Vec<Flag>,
    lines: Vec<String>,
    hidden_lines: usize,
}

enum Load {
    Loading,
    Loaded(Loaded),
    Failed(String),
    /// The session (and its worktree) no longer exists.
    Gone,
}

pub struct ReviewPage {
    stores: Stores,
    session_id: String,
    name: String,
    load: Load,
    show_log: bool,
    _sub: gpui::Subscription,
}

impl EventEmitter<Navigate> for ReviewPage {}
impl EventEmitter<GoBack> for ReviewPage {}

fn parse(diff: &str) -> Loaded {
    let all: Vec<&str> = diff.lines().collect();
    let hidden_lines = all.len().saturating_sub(MAX_DIFF_LINES);
    Loaded {
        files: review::files(diff),
        flags: review::flags(diff),
        lines: all
            .into_iter()
            .take(MAX_DIFF_LINES)
            .map(str::to_string)
            .collect(),
        hidden_lines,
    }
}

fn stage_label(stage: Stage) -> String {
    t(match stage {
        Stage::Merge => "review.stage.merge",
        Stage::Build => "review.stage.build",
        Stage::Gate => "review.stage.gate",
        Stage::Ready => "review.stage.ready",
    })
}

fn line_color(line: &str) -> gpui::Rgba {
    if line.starts_with("+++") || line.starts_with("---") || line.starts_with("diff --git") {
        theme::text()
    } else if line.starts_with('+') {
        theme::success()
    } else if line.starts_with('-') {
        theme::error()
    } else if line.starts_with("@@") {
        theme::accent()
    } else {
        theme::text_muted()
    }
}

fn mono_block(lines: Vec<(String, gpui::Rgba)>) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .p(px(10.0))
        .rounded(px(6.0))
        .bg(theme::bg_app())
        .font_family(theme::FONT)
        .text_size(px(11.5))
        .children(lines.into_iter().map(|(l, c)| div().text_color(c).child(l)))
        .into_any_element()
}

fn heading(text: String) -> gpui::Div {
    div()
        .text_size(px(13.0))
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .child(text)
}

impl ReviewPage {
    pub fn new(stores: Stores, session_id: String, cx: &mut Context<Self>) -> Self {
        let name = stores
            .sessions
            .read(cx)
            .get(&session_id)
            .map(|s| s.name.clone())
            .unwrap_or_default();
        let sub = cx.observe(&stores.review, |_, _, cx| cx.notify());
        let mut page = Self {
            stores,
            session_id,
            name,
            load: Load::Loading,
            show_log: false,
            _sub: sub,
        };
        page.reload(cx);
        page
    }

    /// Re-read the diff of the worktree against `user`.
    fn reload(&mut self, cx: &mut Context<Self>) {
        let wt = self
            .stores
            .sessions
            .read(cx)
            .get(&self.session_id)
            .and_then(|s| s.worktree.clone());
        let Some(wt) = wt else {
            self.load = Load::Gone;
            cx.notify();
            return;
        };
        self.load = Load::Loading;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = runtime::run(async move {
                let base = git::merge_base(&wt.path, USER_BRANCH).await?;
                git::diff_since(&wt.path, &base).await
            })
            .await;
            this.update(cx, |this, cx| {
                this.load = match result {
                    Ok(diff) => Load::Loaded(parse(&diff)),
                    Err(e) => Load::Failed(e),
                };
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn apply(&mut self, cx: &mut Context<Self>) {
        let id = self.session_id.clone();
        self.stores.review.update(cx, |s, cx| s.apply(&id, cx));
    }

    fn ask_discard(&self, cx: &mut Context<Self>) {
        let options = ConfirmOptions::new(
            t("review.discard.title"),
            t_args("review.discard.body", &[("name", &self.name)]),
        )
        .labels(t("review.discard.confirm"), t("common.cancel"));
        let (stores, id) = (self.stores.clone(), self.session_id.clone());
        let page = cx.entity();
        confirm::ask(cx, options, move |answer, _, cx| {
            if answer == Some(true) {
                stores.review.update(cx, |s, cx| s.discard(&id, cx));
                page.update(cx, |_, cx| cx.emit(Navigate(Route::Sessions)));
            }
        });
    }

    fn ask_restart(&self, cx: &mut Context<Self>) {
        let options = ConfirmOptions::new(t("review.restart.title"), t("review.restart.body"))
            .labels(t("review.restart.confirm"), t("common.cancel"));
        let stores = self.stores.clone();
        confirm::ask(cx, options, move |answer, _, cx| {
            if answer == Some(true) {
                stores.review.update(cx, |s, cx| s.restart(cx));
            }
        });
    }

    fn open_session(&self, cx: &mut Context<Self>) {
        let id = self.session_id.clone();
        self.stores.sessions.update(cx, |s, cx| s.select(&id, cx));
        cx.emit(Navigate(Route::Sessions));
    }

    // ---- pieces ------------------------------------------------------------------------------

    fn status_pill(run: Option<&Run>) -> AnyElement {
        let chips = STAGES.iter().map(|&stage| {
            let state = run.map_or(StageState::Pending, |r| r.stage_state(stage));
            let (color, mark) = match state {
                StageState::Pending => (theme::text_muted(), "·"),
                StageState::Active => (theme::accent(), "…"),
                StageState::Done => (theme::success(), "✓"),
                StageState::Failed => (theme::error(), "✗"),
            };
            div()
                .flex()
                .items_center()
                .gap(px(5.0))
                .px(px(10.0))
                .h(px(24.0))
                .rounded(px(12.0))
                .border_1()
                .border_color(color)
                .text_size(px(12.0))
                .text_color(color)
                .child(format!("{mark} {}", stage_label(stage)))
        });
        div()
            .flex()
            .flex_wrap()
            .gap(px(6.0))
            .children(chips)
            .into_any_element()
    }

    fn status_text(run: Option<&Run>) -> Option<(String, gpui::Rgba)> {
        let run = run?;
        Some(match &run.phase {
            RunPhase::Running => (t("review.status.running"), theme::text()),
            RunPhase::Conflict(files) => (
                t_args("review.status.conflict", &[("files", &files.join(", "))]),
                theme::warning(),
            ),
            RunPhase::Failed(f) => {
                let text = match f {
                    Failure::Build => t("review.status.build_failed"),
                    Failure::Tests => t("review.status.tests_failed"),
                    Failure::Smoke => t("review.status.smoke_failed"),
                    Failure::Other(e) => t_args("review.status.error", &[("error", e)]),
                };
                (text, theme::error())
            }
            RunPhase::Ready(_) => (t("review.status.ready"), theme::success()),
        })
    }

    /// The flags banner. Deliberately loud: review is the defense against a weakened gate.
    fn flags_banner(flags: &[Flag]) -> AnyElement {
        div()
            .flex()
            .flex_col()
            .gap(px(6.0))
            .p(px(12.0))
            .rounded(px(6.0))
            .border_2()
            .border_color(theme::warning())
            .bg(theme::accent_soft())
            .child(
                div()
                    .text_size(px(14.0))
                    .font_weight(gpui::FontWeight::BOLD)
                    .text_color(theme::warning())
                    .child(format!("⚠ {}", t("review.flags.title"))),
            )
            .child(div().text_size(px(12.5)).child(t("review.flags.body")))
            .children(flags.iter().map(|f| {
                div()
                    .flex()
                    .gap(px(8.0))
                    .text_size(px(12.0))
                    .child(
                        div()
                            .w(px(190.0))
                            .flex_none()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child(t(f.kind.label_key())),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .font_family(theme::FONT)
                            .child(f.file.clone()),
                    )
            }))
            .into_any_element()
    }

    fn files_list(files: &[FileChange]) -> AnyElement {
        div()
            .flex()
            .flex_col()
            .gap(px(2.0))
            .children(files.iter().map(|f| {
                div()
                    .flex()
                    .gap(px(10.0))
                    .text_size(px(12.0))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .font_family(theme::FONT)
                            .child(f.path.clone()),
                    )
                    .child(
                        div()
                            .text_color(theme::success())
                            .child(format!("+{}", f.added)),
                    )
                    .child(
                        div()
                            .text_color(theme::error())
                            .child(format!("-{}", f.removed)),
                    )
            }))
            .into_any_element()
    }
}

impl Render for ReviewPage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let store = self.stores.review.read(cx);
        let run = store.for_session(&self.session_id);
        let busy = store.running();
        let session_alive = self
            .stores
            .sessions
            .read(cx)
            .get(&self.session_id)
            .is_some();
        let has_changes = matches!(&self.load, Load::Loaded(l) if !l.files.is_empty());
        let can_apply = session_alive && !busy && has_changes;

        let phase = run.map(|r| r.phase.clone());
        let failed = matches!(phase, Some(RunPhase::Failed(_)));
        let conflict = matches!(phase, Some(RunPhase::Conflict(_)));
        let ready = matches!(phase, Some(RunPhase::Ready(_)));
        let has_log = run.is_some_and(|r| !r.log.is_empty());
        let status = Self::status_text(run);
        let log_lines: Vec<(String, gpui::Rgba)> = run
            .map(|r| {
                let skip = r.log.len().saturating_sub(MAX_LOG_SHOWN);
                r.log
                    .iter()
                    .skip(skip)
                    .map(|l| (l.clone(), theme::text_muted()))
                    .collect()
            })
            .unwrap_or_default();
        let pill = Self::status_pill(run);

        let body: Vec<AnyElement> = match &self.load {
            Load::Loading => vec![
                div()
                    .text_color(theme::text_muted())
                    .child(t("review.loading"))
                    .into_any_element(),
            ],
            Load::Gone if ready => Vec::new(),
            Load::Gone => vec![
                div()
                    .text_color(theme::text_muted())
                    .child(t("review.session_gone"))
                    .into_any_element(),
            ],
            Load::Failed(e) => vec![
                div()
                    .text_color(theme::error())
                    .child(t_args("review.load_failed", &[("error", e)]))
                    .into_any_element(),
            ],
            Load::Loaded(l) if l.files.is_empty() => {
                vec![
                    div()
                        .text_color(theme::text_muted())
                        .child(t("review.no_changes"))
                        .into_any_element(),
                ]
            }
            Load::Loaded(l) => {
                let mut out = Vec::new();
                if !l.flags.is_empty() {
                    out.push(Self::flags_banner(&l.flags));
                }
                out.push(
                    heading(t_args(
                        "review.files",
                        &[("count", &l.files.len().to_string())],
                    ))
                    .into_any_element(),
                );
                out.push(Self::files_list(&l.files));
                let mut lines: Vec<_> =
                    l.lines.iter().map(|x| (x.clone(), line_color(x))).collect();
                if l.hidden_lines > 0 {
                    lines.push((
                        t_args("review.truncated", &[("n", &l.hidden_lines.to_string())]),
                        theme::warning(),
                    ));
                }
                out.push(mono_block(lines));
                out
            }
        };

        let buttons = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(8.0))
            .child(
                button(
                    "review-apply",
                    t("review.apply"),
                    if can_apply {
                        ButtonVariant::Classic
                    } else {
                        ButtonVariant::Secondary
                    },
                    cx.listener(|this, _, _, cx| this.apply(cx)),
                )
                .opacity(if can_apply { 1.0 } else { 0.5 }),
            )
            .when(ready, |s| {
                s.child(button(
                    "review-restart",
                    t("review.restart"),
                    ButtonVariant::Classic,
                    cx.listener(|this, _, _, cx| this.ask_restart(cx)),
                ))
            })
            .when(conflict && session_alive, |s| {
                s.child(small_button(
                    "review-open",
                    t("review.open_session"),
                    cx.listener(|this, _, _, cx| this.open_session(cx)),
                ))
            })
            .when(failed && session_alive, |s| {
                s.child(small_button(
                    "review-send-log",
                    t("review.send_log"),
                    cx.listener(|this, _, _, cx| {
                        let id = this.session_id.clone();
                        this.stores.review.update(cx, |s, cx| s.send_log(&id, cx));
                        this.open_session(cx);
                    }),
                ))
            })
            .when(has_log, |s| {
                s.child(small_button(
                    "review-log",
                    if self.show_log {
                        t("review.log.hide")
                    } else {
                        t("review.log.show")
                    },
                    cx.listener(|this, _, _, cx| {
                        this.show_log = !this.show_log;
                        cx.notify();
                    }),
                ))
            })
            .when(session_alive && !busy, |s| {
                s.child(small_button(
                    "review-refresh",
                    t("review.refresh"),
                    cx.listener(|this, _, _, cx| this.reload(cx)),
                ))
                .child(small_button(
                    "review-discard",
                    t("review.discard"),
                    cx.listener(|this, _, _, cx| this.ask_discard(cx)),
                ))
            });

        div()
            .id("review-scroll")
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .overflow_y_scroll()
            .items_center()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .w_full()
                    .max_w(px(1100.0))
                    .p(px(24.0))
                    .gap(px(12.0))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .text_size(px(20.0))
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .child(t("review.title")),
                            )
                            .child(
                                div()
                                    .text_size(px(12.0))
                                    .text_color(theme::text_muted())
                                    .child(self.name.clone()),
                            ),
                    )
                    .child(pill)
                    .when_some(status, |s, (text, color)| {
                        s.child(div().text_size(px(12.5)).text_color(color).child(text))
                    })
                    .child(buttons)
                    .when(self.show_log && has_log, |s| s.child(mono_block(log_lines)))
                    .children(body),
            )
    }
}
