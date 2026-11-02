
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use gpui::Context;
use tokio::sync::Notify;

use crate::backend::workflow::{self, Run, StepOutcome, StepStatus, Workflow};
use crate::model::new_id;
use crate::runtime;

#[derive(Default)]
pub struct WorkflowsStore {
    pub workflows: HashMap<String, Vec<Workflow>>,
    pub runs: HashMap<String, Vec<Run>>,
    active: HashMap<String, Arc<Notify>>,
}

impl WorkflowsStore {
    pub fn load(&mut self, project_id: &str, root: &std::path::Path, cx: &mut Context<Self>) {
        self.workflows
            .insert(project_id.to_string(), workflow::load_workflows(root));
        let mut runs = workflow::load_runs(project_id);
        if let Some(old) = self.runs.get(project_id) {
            for run in &mut runs {
                if self.active.contains_key(&run.id) {
                    if let Some(mem) = old.iter().find(|r| r.id == run.id) {
                        *run = mem.clone();
                    }
                }
            }
        }
        self.runs.insert(project_id.to_string(), runs);
        cx.notify();
    }

    pub fn workflows_of(&self, project_id: &str) -> &[Workflow] {
        self.workflows.get(project_id).map_or(&[], Vec::as_slice)
    }

    pub fn runs_of(&self, project_id: &str) -> &[Run] {
        self.runs.get(project_id).map_or(&[], Vec::as_slice)
    }

    pub fn is_active(&self, run_id: &str) -> bool {
        self.active.contains_key(run_id)
    }

    pub fn workflow_busy(&self, project_id: &str, workflow_id: &str) -> bool {
        self.runs_of(project_id)
            .iter()
            .any(|r| r.workflow_id == workflow_id && self.is_active(&r.id))
    }

    pub fn save(
        &mut self,
        project_id: &str,
        root: &std::path::Path,
        wf: Workflow,
        cx: &mut Context<Self>,
    ) -> std::io::Result<()> {
        workflow::save_workflow(root, &wf)?;
        let list = self.workflows.entry(project_id.to_string()).or_default();
        match list.iter_mut().find(|w| w.id == wf.id) {
            Some(slot) => *slot = wf,
            None => list.push(wf),
        }
        list.sort_by_key(|w| w.name.to_lowercase());
        cx.notify();
        Ok(())
    }

    pub fn delete(
        &mut self,
        project_id: &str,
        root: &std::path::Path,
        id: &str,
        cx: &mut Context<Self>,
    ) -> std::io::Result<()> {
        workflow::delete_workflow(root, id)?;
        if let Some(list) = self.workflows.get_mut(project_id) {
            list.retain(|w| w.id != id);
        }
        cx.notify();
        Ok(())
    }

    pub fn delete_run(&mut self, project_id: &str, run_id: &str, cx: &mut Context<Self>) {
        if self.is_active(run_id) {
            return;
        }
        workflow::delete_run(project_id, run_id);
        if let Some(list) = self.runs.get_mut(project_id) {
            list.retain(|r| r.id != run_id);
        }
        cx.notify();
    }

    pub fn start(
        &mut self,
        project_id: &str,
        root: PathBuf,
        workflow_id: &str,
        input: String,
        program: String,
        cx: &mut Context<Self>,
    ) -> Option<String> {
        if self.workflow_busy(project_id, workflow_id) {
            return None;
        }
        let wf = self
            .workflows_of(project_id)
            .iter()
            .find(|w| w.id == workflow_id)
            .filter(|w| !w.steps.is_empty())?
            .clone();
        let run = Run::new(new_id("r"), &wf, input);
        let id = run.id.clone();
        self.runs
            .entry(project_id.to_string())
            .or_default()
            .insert(0, run);
        self.drive(project_id.to_string(), root, id.clone(), 0, program, cx);
        Some(id)
    }

    pub fn resume(
        &mut self,
        project_id: &str,
        root: PathBuf,
        run_id: &str,
        from: usize,
        program: String,
        cx: &mut Context<Self>,
    ) {
        if self.is_active(run_id) {
            return;
        }
        let current = {
            let Some(run) = self.runs_of(project_id).iter().find(|r| r.id == run_id) else {
                return;
            };
            if from >= run.steps.len() || run.steps.is_empty() {
                return;
            }
            if self.workflow_busy(project_id, &run.workflow_id) {
                return;
            }
            self.workflows_of(project_id)
                .iter()
                .find(|w| w.id == run.workflow_id)
                .cloned()
        };
        let Some(run) = self
            .runs
            .get_mut(project_id)
            .and_then(|l| l.iter_mut().find(|r| r.id == run_id))
        else {
            return;
        };
        for i in from..run.steps.len() {
            if let Some(step) = current.as_ref().and_then(|w| w.steps.get(i)) {
                run.steps[i] = step.clone();
            }
            run.results[i] = Default::default();
        }
        self.drive(
            project_id.to_string(),
            root,
            run_id.to_string(),
            from,
            program,
            cx,
        );
    }

    pub fn cancel(&mut self, run_id: &str) {
        if let Some(n) = self.active.get(run_id) {
            n.notify_one();
        }
    }

    fn run_mut(&mut self, project_id: &str, run_id: &str) -> Option<&mut Run> {
        self.runs
            .get_mut(project_id)?
            .iter_mut()
            .find(|r| r.id == run_id)
    }

    fn drive(
        &mut self,
        project_id: String,
        root: PathBuf,
        run_id: String,
        from: usize,
        program: String,
        cx: &mut Context<Self>,
    ) {
        let cancel = Arc::new(Notify::new());
        self.active.insert(run_id.clone(), cancel.clone());
        cx.notify();
        cx.spawn(async move |this, cx| {
            let mut index = from;
            loop {
                let begun = this
                    .update(cx, |s, cx| s.begin_step(&project_id, &run_id, index, cx))
                    .ok()
                    .flatten();
                let Some((prompt, model)) = begun else { break };
                let (prog, dir, c) = (program.clone(), root.clone(), cancel.clone());
                let outcome = runtime::run(async move {
                    workflow::run_step(&prog, &dir, &model, &prompt, c).await
                })
                .await;
                let go_on = this
                    .update(cx, |s, cx| {
                        s.finish_step(&project_id, &run_id, index, outcome, cx)
                    })
                    .unwrap_or(false);
                if !go_on {
                    break;
                }
                index += 1;
            }
            this.update(cx, |s, cx| {
                s.active.remove(&run_id);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn begin_step(
        &mut self,
        project_id: &str,
        run_id: &str,
        index: usize,
        cx: &mut Context<Self>,
    ) -> Option<(String, String)> {
        let run = self.run_mut(project_id, run_id)?;
        if index >= run.steps.len() {
            return None;
        }
        let prompt = run.prompt_for(index);
        run.results[index].status = StepStatus::Running;
        run.results[index].prompt = prompt.clone();
        run.results[index].output.clear();
        let model = run.steps[index].model.clone();
        workflow::save_run(project_id, run);
        cx.notify();
        Some((prompt, model))
    }

    fn finish_step(
        &mut self,
        project_id: &str,
        run_id: &str,
        index: usize,
        outcome: StepOutcome,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(run) = self.run_mut(project_id, run_id) else {
            return false;
        };
        let (status, output) = match outcome {
            StepOutcome::Done(out) => (StepStatus::Done, out),
            StepOutcome::Failed(e) => (StepStatus::Failed, e),
            StepOutcome::Cancelled => (StepStatus::Cancelled, String::new()),
        };
        run.results[index].status = status;
        run.results[index].output = output;
        let more = status == StepStatus::Done && index + 1 < run.steps.len();
        workflow::save_run(project_id, run);
        cx.notify();
        more
    }
}
