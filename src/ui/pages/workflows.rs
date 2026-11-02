
use std::collections::BTreeSet;

use gpui::{
    AnyElement, Context, Entity, EventEmitter, InteractiveElement, IntoElement, ParentElement,
    Render, SharedString, StatefulInteractiveElement, Styled, Subscription, Window, div,
    prelude::*, px,
};

use super::skills::{card, icon_button, muted_note, row_base};
use crate::backend::workflow::{self, Run, Step, StepStatus, Workflow};
use crate::i18n::{t, t_args};
use crate::model::new_id;
use crate::route::{GoBack, Navigate, Route};
use crate::state::Stores;
use crate::state::sessions;
use crate::ui::components::button::{ButtonVariant, button, small_button};
use crate::ui::components::text_area::TextArea;
use crate::ui::components::text_input::TextInput;
use crate::ui::confirm::{self, ConfirmOptions};
use crate::ui::icons::{Icon, icon};
use crate::ui::{theme, toast};

const SHOWN_CHARS: usize = 12_000;

struct StepDraft {
    name: Entity<TextInput>,
    model: Entity<TextInput>,
    skills: BTreeSet<String>,
    prompt: Entity<TextArea>,
}

struct Draft {
    id: String,
    name: Entity<TextInput>,
    steps: Vec<StepDraft>,
}

pub struct WorkflowsPage {
    stores: Stores,
    project_id: String,
    draft: Option<Draft>,
    target: Option<String>,
    task: Entity<TextInput>,
    viewing: Option<String>,
    _subs: Vec<Subscription>,
}

impl EventEmitter<Navigate> for WorkflowsPage {}
impl EventEmitter<GoBack> for WorkflowsPage {}

fn format_time(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02}",
        rem / 3600,
        rem % 3600 / 60
    )
}

fn field(input: &Entity<TextInput>) -> impl IntoElement {
    div()
        .flex()
        .flex_1()
        .min_w_0()
        .h(px(26.0))
        .px(px(8.0))
        .rounded(px(4.0))
        .border_1()
        .border_color(theme::border_subtle())
        .bg(theme::bg_app())
        .child(input.clone())
}

fn status_label(status: StepStatus) -> (String, gpui::Rgba) {
    match status {
        StepStatus::Pending => (t("workflows.status.pending"), theme::text_muted()),
        StepStatus::Running => (t("workflows.status.running"), theme::accent()),
        StepStatus::Done => (t("workflows.status.done"), theme::success()),
        StepStatus::Failed => (t("workflows.status.failed"), theme::error()),
        StepStatus::Cancelled => (t("workflows.status.cancelled"), theme::warning()),
    }
}

fn run_status(run: &Run) -> StepStatus {
    if run.running() {
        StepStatus::Running
    } else if run.finished() {
        StepStatus::Done
    } else if run.results.iter().any(|r| r.status == StepStatus::Failed) {
        StepStatus::Failed
    } else if run
        .results
        .iter()
        .any(|r| r.status == StepStatus::Cancelled)
    {
        StepStatus::Cancelled
    } else {
        StepStatus::Pending
    }
}

fn clip(text: &str) -> String {
    if text.chars().count() <= SHOWN_CHARS {
        return text.to_string();
    }
    let tail: String = text
        .chars()
        .skip(text.chars().count() - SHOWN_CHARS)
        .collect();
    format!("…{tail}")
}

impl WorkflowsPage {
    pub fn new(stores: Stores, project_id: String, cx: &mut Context<Self>) -> Self {
        let task = cx.new(|cx| TextInput::new(t("workflows.task_placeholder"), cx));
        let mut subs = vec![cx.observe(&stores.workflows, |_, _, cx| cx.notify())];
        subs.push(cx.observe(&stores.skills, |_, _, cx| cx.notify()));
        stores.skills.update(cx, |s, cx| s.refresh(cx));
        let mut this = Self {
            stores,
            project_id,
            draft: None,
            target: None,
            task,
            viewing: None,
            _subs: subs,
        };
        this.reload(cx);
        this.target = this
            .stores
            .workflows
            .read(cx)
            .workflows_of(&this.project_id)
            .first()
            .map(|w| w.id.clone());
        this
    }

    fn root(&self, cx: &Context<Self>) -> Option<std::path::PathBuf> {
        self.stores.projects.read(cx).path_of(&self.project_id)
    }

    fn reload(&mut self, cx: &mut Context<Self>) {
        let Some(root) = self.root(cx) else { return };
        let id = self.project_id.clone();
        self.stores
            .workflows
            .update(cx, |s, cx| s.load(&id, &root, cx));
    }

    fn make_draft(&self, id: String, name: &str, steps: &[Step], cx: &mut Context<Self>) -> Draft {
        let name = {
            let n = name.to_string();
            cx.new(|cx| TextInput::new(t("workflows.name_placeholder"), cx).with_value(n))
        };
        let steps = steps.iter().map(|s| self.step_draft(s, cx)).collect();
        Draft { id, name, steps }
    }

    fn step_draft(&self, step: &Step, cx: &mut Context<Self>) -> StepDraft {
        let (n, m, p) = (step.name.clone(), step.model.clone(), step.prompt.clone());
        StepDraft {
            name: cx.new(|cx| TextInput::new(t("workflows.step_name"), cx).with_value(n)),
            model: cx.new(|cx| TextInput::new(t("workflows.model_placeholder"), cx).with_value(m)),
            skills: step.skills.iter().cloned().collect(),
            prompt: cx.new(|cx| {
                let mut a = TextArea::new(cx);
                a.set_value(&p, cx);
                a
            }),
        }
    }

    fn new_from(&mut self, name: &str, steps: Vec<Step>, cx: &mut Context<Self>) {
        self.draft = Some(self.make_draft(new_id("w"), name, &steps, cx));
        cx.notify();
    }

    fn edit(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(wf) = self
            .stores
            .workflows
            .read(cx)
            .workflows_of(&self.project_id)
            .iter()
            .find(|w| w.id == id)
            .cloned()
        else {
            return;
        };
        self.draft = Some(self.make_draft(wf.id, &wf.name, &wf.steps, cx));
        cx.notify();
    }

    fn add_step(&mut self, cx: &mut Context<Self>) {
        let sd = self.step_draft(&Step::default(), cx);
        if let Some(d) = self.draft.as_mut() {
            d.steps.push(sd);
        }
        cx.notify();
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        let Some(draft) = self.draft.as_ref() else {
            return;
        };
        let name = draft.name.read(cx).value().trim().to_string();
        if name.is_empty() {
            toast::error(cx, t("workflows.name_required"));
            return;
        }
        if draft.steps.is_empty() {
            toast::error(cx, t("workflows.steps_required"));
            return;
        }
        let steps: Vec<Step> = draft
            .steps
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let step_name = s.name.read(cx).value().trim().to_string();
                Step {
                    name: if step_name.is_empty() {
                        format!("Step {}", i + 1)
                    } else {
                        step_name
                    },
                    prompt: s.prompt.read(cx).value(),
                    model: s.model.read(cx).value().trim().to_string(),
                    skills: s.skills.iter().cloned().collect(),
                }
            })
            .collect();
        let wf = Workflow {
            id: draft.id.clone(),
            name: name.clone(),
            steps,
        };
        let Some(root) = self.root(cx) else { return };
        let (pid, wid) = (self.project_id.clone(), wf.id.clone());
        let result = self
            .stores
            .workflows
            .update(cx, |s, cx| s.save(&pid, &root, wf, cx));
        match result {
            Ok(()) => {
                toast::success(cx, t_args("workflows.saved", &[("name", &name)]));
                self.draft = None;
                if self.target.is_none() {
                    self.target = Some(wid);
                }
            }
            Err(e) => toast::error(
                cx,
                t_args("workflows.save_failed", &[("error", &e.to_string())]),
            ),
        }
        cx.notify();
    }

    fn confirm_delete(&self, id: String, name: String, cx: &mut Context<Self>) {
        let options = ConfirmOptions::new(
            t("workflows.delete.title"),
            t_args("workflows.delete.body", &[("name", &name)]),
        )
        .labels(t("common.delete"), t("common.cancel"));
        let page = cx.entity();
        confirm::ask(cx, options, move |answer, _, cx| {
            if answer != Some(true) {
                return;
            }
            page.update(cx, |this, cx| {
                let Some(root) = this.root(cx) else { return };
                let pid = this.project_id.clone();
                let result = this
                    .stores
                    .workflows
                    .update(cx, |s, cx| s.delete(&pid, &root, &id, cx));
                match result {
                    Ok(()) => {
                        if this.target.as_deref() == Some(&id) {
                            this.target = None;
                        }
                        if this.draft.as_ref().is_some_and(|d| d.id == id) {
                            this.draft = None;
                        }
                    }
                    Err(e) => toast::error(
                        cx,
                        t_args("workflows.save_failed", &[("error", &e.to_string())]),
                    ),
                }
                cx.notify();
            });
        });
    }

    fn start_run(&mut self, cx: &mut Context<Self>) {
        let (Some(root), Some(target)) = (self.root(cx), self.target.clone()) else {
            return;
        };
        let input = self.task.read(cx).value().trim().to_string();
        let program = self.stores.prefs.read(cx).prefs.program().to_string();
        let pid = self.project_id.clone();
        let started = self
            .stores
            .workflows
            .update(cx, |s, cx| s.start(&pid, root, &target, input, program, cx));
        match started {
            Some(id) => self.viewing = Some(id),
            None => toast::error(cx, t("workflows.cannot_start")),
        }
        cx.notify();
    }

    fn resume_run(&mut self, run_id: &str, from: usize, cx: &mut Context<Self>) {
        let Some(root) = self.root(cx) else { return };
        let program = self.stores.prefs.read(cx).prefs.program().to_string();
        let (pid, rid) = (self.project_id.clone(), run_id.to_string());
        self.stores
            .workflows
            .update(cx, |s, cx| s.resume(&pid, root, &rid, from, program, cx));
    }

    fn open_as_session(&self, prompt: &str, cx: &mut Context<Self>) {
        sessions::begin_new_input(&self.project_id, Some(prompt.to_string()), cx);
        cx.emit(Navigate(Route::Sessions));
    }

    fn list_card(&self, cx: &mut Context<Self>) -> AnyElement {
        let workflows = self
            .stores
            .workflows
            .read(cx)
            .workflows_of(&self.project_id)
            .to_vec();
        let mut body: Vec<AnyElement> = Vec::new();
        if workflows.is_empty() {
            body.push(muted_note(t("workflows.empty").into()).into_any_element());
        }
        for (i, wf) in workflows.iter().enumerate() {
            let (pick, edit, del) = (wf.id.clone(), wf.id.clone(), wf.id.clone());
            let del_name = wf.name.clone();
            let selected = self.target.as_deref() == Some(wf.id.as_str());
            body.push(
                row_base(i)
                    .child(
                        div()
                            .id(SharedString::from(format!("wf-{}", wf.id)))
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .cursor_pointer()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.target = Some(pick.clone());
                                cx.notify();
                            }))
                            .child(
                                div()
                                    .truncate()
                                    .text_size(px(13.0))
                                    .text_color(if selected {
                                        theme::accent()
                                    } else {
                                        theme::text()
                                    })
                                    .child(wf.name.clone()),
                            )
                            .child(
                                div()
                                    .truncate()
                                    .text_size(px(11.0))
                                    .text_color(theme::text_muted())
                                    .child(
                                        wf.steps
                                            .iter()
                                            .map(|s| s.name.as_str())
                                            .collect::<Vec<_>>()
                                            .join(" → "),
                                    ),
                            ),
                    )
                    .child(icon_button(
                        SharedString::from(format!("edit-wf-{}", wf.id)),
                        Icon::Pencil,
                        cx.listener(move |this, _, _, cx| this.edit(&edit, cx)),
                    ))
                    .child(icon_button(
                        SharedString::from(format!("rm-wf-{}", wf.id)),
                        Icon::Trash,
                        cx.listener(move |this, _, _, cx| {
                            this.confirm_delete(del.clone(), del_name.clone(), cx)
                        }),
                    ))
                    .into_any_element(),
            );
        }
        let mut templates = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(6.0))
            .mt(px(6.0));
        templates = templates.child(
            div()
                .text_size(px(11.0))
                .text_color(theme::text_muted())
                .child(t("workflows.from_template")),
        );
        for (i, (name, steps)) in workflow::templates().into_iter().enumerate() {
            templates = templates.child(small_button(
                SharedString::from(format!("tpl-{i}")),
                name,
                cx.listener(move |this, _, _, cx| this.new_from(name, steps.clone(), cx)),
            ));
        }
        body.push(templates.into_any_element());
        let new = small_button(
            "wf-new",
            t("workflows.new"),
            cx.listener(|this, _, _, cx| this.new_from("", vec![Step::default()], cx)),
        )
        .into_any_element();
        div()
            .flex()
            .child(card(
                &t("workflows.title"),
                Some(workflows.len()),
                Some(new),
                body,
            ))
            .into_any_element()
    }

    fn editor_card(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let draft = self.draft.as_ref()?;
        let installed = self.stores.skills.read(cx).snapshot.installed.clone();
        let mut body: Vec<AnyElement> = vec![
            div()
                .flex()
                .items_center()
                .gap(px(8.0))
                .child(
                    div()
                        .text_size(px(12.0))
                        .text_color(theme::text_muted())
                        .child(t("workflows.name")),
                )
                .child(field(&draft.name))
                .into_any_element(),
            muted_note(t("workflows.variables_hint").into()).into_any_element(),
        ];
        let count = draft.steps.len();
        for (i, step) in draft.steps.iter().enumerate() {
            let chips = installed.iter().map(|skill| {
                let on = step.skills.contains(skill);
                let name = skill.clone();
                div()
                    .id(SharedString::from(format!("chip-{i}-{skill}")))
                    .px(px(8.0))
                    .py(px(2.0))
                    .rounded(px(10.0))
                    .border_1()
                    .border_color(if on {
                        theme::accent()
                    } else {
                        theme::border_subtle()
                    })
                    .when(on, |s| s.bg(theme::accent_soft()))
                    .text_size(px(11.0))
                    .text_color(theme::text())
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(s) = this.draft.as_mut().and_then(|d| d.steps.get_mut(i)) {
                            if !s.skills.remove(&name) {
                                s.skills.insert(name.clone());
                            }
                        }
                        cx.notify();
                    }))
                    .child(skill.clone())
            });
            body.push(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.0))
                    .p(px(8.0))
                    .rounded(px(6.0))
                    .border_1()
                    .border_color(theme::border_subtle())
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .child(
                                div()
                                    .text_size(px(12.0))
                                    .text_color(theme::text_muted())
                                    .child(format!("{}.", i + 1)),
                            )
                            .child(field(&step.name))
                            .child(field(&step.model))
                            .child(icon_button(
                                SharedString::from(format!("step-up-{i}")),
                                Icon::ChevronDown,
                                cx.listener(move |this, _, _, cx| {
                                    if let Some(d) = this.draft.as_mut() {
                                        if i + 1 < d.steps.len() {
                                            d.steps.swap(i, i + 1);
                                        }
                                    }
                                    cx.notify();
                                }),
                            ))
                            .child(icon_button(
                                SharedString::from(format!("step-rm-{i}")),
                                Icon::Trash,
                                cx.listener(move |this, _, _, cx| {
                                    if let Some(d) = this.draft.as_mut() {
                                        if i < d.steps.len() {
                                            d.steps.remove(i);
                                        }
                                    }
                                    cx.notify();
                                }),
                            )),
                    )
                    .child(
                        div()
                            .flex()
                            .h(px(130.0))
                            .rounded(px(4.0))
                            .border_1()
                            .border_color(theme::border_subtle())
                            .bg(theme::bg_app())
                            .child(step.prompt.clone()),
                    )
                    .when(!installed.is_empty(), |s| {
                        s.child(
                            div()
                                .flex()
                                .flex_wrap()
                                .items_center()
                                .gap(px(6.0))
                                .child(
                                    div()
                                        .text_size(px(11.0))
                                        .text_color(theme::text_muted())
                                        .child(t("workflows.skills")),
                                )
                                .children(chips),
                        )
                    })
                    .into_any_element(),
            );
        }
        let _ = count;
        body.push(
            div()
                .flex()
                .items_center()
                .gap(px(8.0))
                .child(small_button(
                    "wf-add-step",
                    t("workflows.add_step"),
                    cx.listener(|this, _, _, cx| this.add_step(cx)),
                ))
                .child(div().flex_1())
                .child(button(
                    "wf-cancel-edit",
                    t("common.cancel"),
                    ButtonVariant::Secondary,
                    cx.listener(|this, _, _, cx| {
                        this.draft = None;
                        cx.notify();
                    }),
                ))
                .child(button(
                    "wf-save",
                    t("workflows.save"),
                    ButtonVariant::Classic,
                    cx.listener(|this, _, _, cx| this.save(cx)),
                ))
                .into_any_element(),
        );
        Some(
            div()
                .flex()
                .child(card(&t("workflows.editor"), None, None, body))
                .into_any_element(),
        )
    }

    fn run_card(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let store = self.stores.workflows.read(cx);
        let wf = store
            .workflows_of(&self.project_id)
            .iter()
            .find(|w| Some(&w.id) == self.target.as_ref())?;
        let busy = store.workflow_busy(&self.project_id, &wf.id);
        let body: Vec<AnyElement> = vec![
            div()
                .flex()
                .items_center()
                .gap(px(8.0))
                .child(field(&self.task))
                .child(button(
                    "wf-run",
                    if busy {
                        t("workflows.running")
                    } else {
                        t("workflows.run")
                    },
                    ButtonVariant::Classic,
                    cx.listener(|this, _, _, cx| this.start_run(cx)),
                ))
                .into_any_element(),
        ];
        Some(
            div()
                .flex()
                .child(card(
                    &t_args("workflows.run_title", &[("name", &wf.name)]),
                    None,
                    None,
                    body,
                ))
                .into_any_element(),
        )
    }

    fn history_card(&self, cx: &mut Context<Self>) -> AnyElement {
        let store = self.stores.workflows.read(cx);
        let runs = store.runs_of(&self.project_id).to_vec();
        let mut body: Vec<AnyElement> = Vec::new();
        if runs.is_empty() {
            body.push(muted_note(t("workflows.no_runs").into()).into_any_element());
        }
        for (i, run) in runs.iter().enumerate() {
            let (label, color) = status_label(run_status(run));
            let (pick, del) = (run.id.clone(), run.id.clone());
            let active = store.is_active(&run.id);
            let viewing = self.viewing.as_deref() == Some(run.id.as_str());
            body.push(
                row_base(i)
                    .child(
                        div()
                            .id(SharedString::from(format!("run-{}", run.id)))
                            .flex()
                            .flex_1()
                            .min_w_0()
                            .items_center()
                            .gap(px(8.0))
                            .cursor_pointer()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.viewing = Some(pick.clone());
                                cx.notify();
                            }))
                            .child(
                                div()
                                    .truncate()
                                    .flex_1()
                                    .text_size(px(13.0))
                                    .text_color(if viewing {
                                        theme::accent()
                                    } else {
                                        theme::text()
                                    })
                                    .child(format!(
                                        "{} · {}",
                                        run.workflow_name,
                                        format_time(run.started)
                                    )),
                            )
                            .child(div().text_size(px(11.0)).text_color(color).child(label)),
                    )
                    .when(!active, |s| {
                        s.child(icon_button(
                            SharedString::from(format!("rm-run-{}", run.id)),
                            Icon::Trash,
                            cx.listener(move |this, _, _, cx| {
                                let pid = this.project_id.clone();
                                if this.viewing.as_deref() == Some(del.as_str()) {
                                    this.viewing = None;
                                }
                                this.stores
                                    .workflows
                                    .update(cx, |s, cx| s.delete_run(&pid, &del, cx));
                            }),
                        ))
                    })
                    .into_any_element(),
            );
        }
        if let Some(detail) = self.run_detail(&runs, cx) {
            body.push(detail);
        }
        div()
            .flex()
            .child(card(&t("workflows.history"), Some(runs.len()), None, body))
            .into_any_element()
    }

    fn run_detail(&self, runs: &[Run], cx: &mut Context<Self>) -> Option<AnyElement> {
        let run = runs.iter().find(|r| Some(&r.id) == self.viewing.as_ref())?;
        let active = self.stores.workflows.read(cx).is_active(&run.id);
        let mut col = div()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .mt(px(10.0))
            .pt(px(10.0))
            .border_t_1()
            .border_color(theme::border_subtle());
        if !run.input.is_empty() {
            col = col.child(
                div()
                    .text_size(px(12.0))
                    .text_color(theme::text_secondary())
                    .child(t_args("workflows.task", &[("input", &run.input)])),
            );
        }
        let mut actions = div().flex().items_center().gap(px(8.0));
        if active {
            let rid = run.id.clone();
            actions = actions.child(small_button(
                "run-cancel",
                t("workflows.cancel"),
                cx.listener(move |this, _, _, cx| {
                    this.stores.workflows.update(cx, |s, _| s.cancel(&rid));
                }),
            ));
        } else if let Some(from) = run.resume_point() {
            let rid = run.id.clone();
            actions = actions.child(small_button(
                "run-resume",
                t_args("workflows.resume", &[("n", &(from + 1).to_string())]),
                cx.listener(move |this, _, _, cx| this.resume_run(&rid, from, cx)),
            ));
        }
        col = col.child(actions);
        for (i, step) in run.steps.iter().enumerate() {
            let result = &run.results[i];
            let (label, color) = status_label(result.status);
            let (retry_id, sess_prompt) = (run.id.clone(), result.prompt.clone());
            let shown = clip(&result.output);
            col = col.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .child(
                                div()
                                    .flex_1()
                                    .truncate()
                                    .text_size(px(12.5))
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .child(format!("{}. {}", i + 1, step.name)),
                            )
                            .child(div().text_size(px(11.0)).text_color(color).child(label))
                            .when(!active && result.status != StepStatus::Pending, |s| {
                                s.child(small_button(
                                    SharedString::from(format!("retry-{i}")),
                                    t("workflows.retry"),
                                    cx.listener(move |this, _, _, cx| {
                                        this.resume_run(&retry_id, i, cx)
                                    }),
                                ))
                            })
                            .when(!result.prompt.is_empty(), |s| {
                                s.child(small_button(
                                    SharedString::from(format!("open-{i}")),
                                    t("workflows.open_session"),
                                    cx.listener(move |this, _, _, cx| {
                                        this.open_as_session(&sess_prompt, cx)
                                    }),
                                ))
                            }),
                    )
                    .when(!shown.is_empty(), |s| {
                        s.child(
                            div()
                                .id(SharedString::from(format!("out-{}-{i}", run.id)))
                                .max_h(px(240.0))
                                .overflow_y_scroll()
                                .p(px(8.0))
                                .rounded(px(4.0))
                                .border_1()
                                .border_color(if result.status == StepStatus::Failed {
                                    theme::error()
                                } else {
                                    theme::border_subtle()
                                })
                                .text_size(px(12.0))
                                .text_color(theme::text_secondary())
                                .child(shown),
                        )
                    }),
            );
        }
        Some(col.into_any_element())
    }
}

impl Render for WorkflowsPage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let list = self.list_card(cx);
        let editor = self.editor_card(cx);
        let run = self.run_card(cx);
        let history = self.history_card(cx);
        div()
            .id("workflows-scroll")
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
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .text_size(px(20.0))
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .child(t("workflows.title")),
                            )
                            .child(
                                div()
                                    .id("workflows-refresh")
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .size(px(26.0))
                                    .rounded(px(6.0))
                                    .cursor_pointer()
                                    .hover(|s| s.bg(theme::hover()))
                                    .on_click(cx.listener(|this, _, _, cx| this.reload(cx)))
                                    .child(icon(Icon::Refresh, px(15.0)).text_color(theme::text())),
                            ),
                    )
                    .child(list)
                    .children(editor)
                    .children(run)
                    .child(history),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_unix_time() {
        assert_eq!(format_time(0), "1970-01-01 00:00");
        assert_eq!(format_time(1_700_000_000), "2023-11-14 22:13");
    }

    #[test]
    fn clip_keeps_the_tail() {
        let long = "a".repeat(SHOWN_CHARS + 5);
        assert_eq!(clip(&long).chars().count(), SHOWN_CHARS + 1);
        assert_eq!(clip("short"), "short");
    }
}
