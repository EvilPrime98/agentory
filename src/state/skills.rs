//! Skills of the selected project, through the built-in `spm`: read from disk, mutate through `backend::spm`.

use std::future::Future;
use std::path::PathBuf;

use gpui::{Context, SharedString};

use crate::backend::spm::{self, AvailableSkill, Snapshot};
use crate::model::DoctorReport;
use crate::runtime;
use crate::i18n::{t, t_args};
use crate::ui::toast;

#[derive(Default)]
pub struct SkillsStore {
    pub project: Option<PathBuf>,
    pub snapshot: Snapshot,
    pub doctor: Option<DoctorReport>,
    /// Skills in the repo pool (`spm available`), installed ones included.
    pub available: Vec<AvailableSkill>,
    /// Why the last `spm available` failed.
    pub available_error: Option<SharedString>,
    /// What is running right now (`"Installing foo…"`); mutations are serialized.
    pub busy: Option<SharedString>,
    pub revision: u64,
    /// Bumped when the project changes so a stale response is dropped.
    generation: u64,
}

impl SkillsStore {
    /// Point the store at a project (or none) and reload.
    pub fn set_project(&mut self, project: Option<PathBuf>, cx: &mut Context<Self>) {
        if self.project == project {
            return;
        }
        self.project = project;
        self.generation += 1;
        self.snapshot = Snapshot::default();
        self.doctor = None;
        self.available.clear();
        self.available_error = None;
        self.revision += 1;
        cx.notify();
        self.refresh(cx);
    }

    /// Re-read `spm.json` and `.claude/skills`, and list the skills in the repo pool.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        self.reload_snapshot(cx);
        self.refresh_available(cx);
    }

    /// The repo pool listing, only for projects that have an `spm.json`.
    fn refresh_available(&mut self, cx: &mut Context<Self>) {
        let Some(dir) = self.project.clone() else {
            return;
        };
        if !dir.join("spm.json").is_file() {
            return;
        }
        let generation = self.generation;
        cx.spawn(async move |this, cx| {
            let result = runtime::run(async move { spm::list_available(&dir).await }).await;
            this.update(cx, |s, cx| {
                if s.generation != generation {
                    return;
                }
                match result {
                    Ok(list) => {
                        s.available = list;
                        s.available_error = None;
                    }
                    Err(e) => {
                        tracing::warn!("spm available failed: {e}");
                        s.available.clear();
                        s.available_error =
                            Some(e.lines().next().unwrap_or_default().to_string().into());
                    }
                }
                s.revision += 1;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn reload_snapshot(&mut self, cx: &mut Context<Self>) {
        let Some(dir) = self.project.clone() else {
            return;
        };
        let generation = self.generation;
        cx.spawn(async move |this, cx| {
            let snapshot = runtime::run(async move { spm::read_snapshot(&dir) }).await;
            this.update(cx, |s, cx| {
                if s.generation == generation {
                    s.snapshot = snapshot;
                    s.revision += 1;
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// Run an `spm` command, then reload. The error (stderr) is shown as a toast.
    fn mutate<F, Fut>(
        &mut self,
        label: String,
        refetch_available: bool,
        cx: &mut Context<Self>,
        call: F,
    ) where
        F: FnOnce(PathBuf) -> Fut + Send + 'static,
        Fut: Future<Output = Result<String, String>> + Send + 'static,
    {
        let Some(dir) = self.project.clone() else {
            return;
        };
        if self.busy.is_some() {
            return;
        }
        self.busy = Some(label.clone().into());
        cx.notify();
        let generation = self.generation;
        cx.spawn(async move |this, cx| {
            let result = runtime::run(call(dir)).await;
            let current = this
                .update(cx, |s, cx| {
                    s.busy = None;
                    cx.notify();
                    if s.generation == generation {
                        s.reload_snapshot(cx);
                        if refetch_available {
                            s.refresh_available(cx);
                        }
                    }
                    s.generation == generation
                })
                .unwrap_or(false);
            if let Err(e) = result {
                tracing::warn!("spm ({label}) failed: {e}");
                cx.update(|cx| toast::error(cx, e));
            } else if current {
                cx.update(|cx| toast::success(cx, t_args("skills.done", &[("label", &label)])));
            }
        })
        .detach();
    }

    pub fn init(&mut self, repo: Option<String>, cx: &mut Context<Self>) {
        self.mutate(
            t("skills.busy.init"),
            true,
            cx,
            move |dir| async move { spm::init(&dir, repo.as_deref(), true).await },
        );
    }

    /// Turn isolation from global skills on or off by re-running `spm init`.
    pub fn set_isolate(&mut self, isolate: bool, cx: &mut Context<Self>) {
        let label = if isolate {
            t("skills.busy.isolate")
        } else {
            t("skills.busy.allow_global")
        };
        self.mutate(label, false, cx, move |dir| async move {
            spm::init(&dir, None, isolate).await
        });
    }

    pub fn install(&mut self, names: Vec<String>, cx: &mut Context<Self>) {
        if names.is_empty() {
            return;
        }
        let label = t_args("skills.busy.install", &[("names", &names.join(", "))]);
        self.mutate(label, false, cx, move |dir| async move {
            spm::install(&dir, &names).await
        });
    }

    pub fn uninstall(&mut self, name: String, cx: &mut Context<Self>) {
        let label = t_args("skills.busy.uninstall", &[("name", &name)]);
        self.mutate(label, false, cx, move |dir| async move {
            spm::uninstall(&dir, &name).await
        });
    }

    pub fn update_all(&mut self, cx: &mut Context<Self>) {
        self.mutate(t("skills.busy.update"), false, cx, |dir| async move {
            spm::update(&dir).await
        });
    }

    pub fn add_repo(&mut self, repo: String, cx: &mut Context<Self>) {
        let label = t_args("skills.busy.add_repo", &[("repo", &repo)]);
        self.mutate(label, true, cx, move |dir| async move {
            spm::repo_add(&dir, &repo).await
        });
    }

    pub fn remove_repo(&mut self, repo: String, cx: &mut Context<Self>) {
        let label = t_args("skills.busy.remove_repo", &[("repo", &repo)]);
        self.mutate(label, true, cx, move |dir| async move {
            spm::repo_rm(&dir, &repo).await
        });
    }

    /// The context-cost report.
    pub fn run_doctor(&mut self, cx: &mut Context<Self>) {
        let Some(dir) = self.project.clone() else {
            return;
        };
        if self.busy.is_some() {
            return;
        }
        self.busy = Some(t("skills.busy.measure").into());
        cx.notify();
        let generation = self.generation;
        cx.spawn(async move |this, cx| {
            let result = runtime::run(async move { spm::doctor(&dir).await }).await;
            let error = this
                .update(cx, |s, cx| {
                    s.busy = None;
                    let error = match result {
                        Ok(report) if s.generation == generation => {
                            s.doctor = Some(report);
                            s.revision += 1;
                            None
                        }
                        Ok(_) => None,
                        Err(e) => Some(e),
                    };
                    cx.notify();
                    error
                })
                .ok()
                .flatten();
            if let Some(e) = error {
                cx.update(|cx| toast::error(cx, e));
            }
        })
        .detach();
    }
}
