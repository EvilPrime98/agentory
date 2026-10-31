//! Skills of the selected project, through the built-in `spm`: read from disk, mutate through `backend::spm`.

use std::future::Future;
use std::path::PathBuf;

use gpui::{Context, SharedString};

use crate::backend::spm::{self, AvailableSkill, Hit, Prepared, Review, Snapshot};
use crate::model::DoctorReport;
use crate::runtime;
use crate::i18n::{t, t_args};
use crate::ui::confirm::{self, ConfirmOptions};
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
    /// Market search results.
    pub hits: Vec<Hit>,
    /// Why the last market search failed.
    pub search_error: Option<SharedString>,
    /// A market search is in flight.
    pub searching: bool,
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
        self.hits.clear();
        self.search_error = None;
        self.searching = false;
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

    /// Install skills from the pool, or into the global folder with `global`. A `market:skill`
    /// reference goes through the review dialog instead, one at a time.
    pub fn install(&mut self, names: Vec<String>, global: bool, cx: &mut Context<Self>) {
        if names.is_empty() {
            return;
        }
        let (markets, plain): (Vec<_>, Vec<_>) = names
            .iter()
            .cloned()
            .partition(|n| spm::parse_market_ref(n).is_some());
        if let Some(first) = markets.first() {
            if markets.len() > 1 || !plain.is_empty() {
                self.toast_error(t("skills.one_market_at_a_time"), cx);
                return;
            }
            if let Some((market, id)) = spm::parse_market_ref(first) {
                self.review_install(market.to_string(), id.to_string(), global, cx);
            }
            return;
        }
        let label = t_args("skills.busy.install", &[("names", &names.join(", "))]);
        self.mutate(label, false, cx, move |dir| async move {
            if global {
                spm::install_global(&dir, &names).await
            } else {
                spm::install(&dir, &names).await
            }
        });
    }

    fn toast_error(&self, text: String, cx: &mut Context<Self>) {
        toast::error(cx, text);
    }

    /// Search the markets; the hits land in `hits`.
    pub fn search_market(&mut self, term: String, cx: &mut Context<Self>) {
        let term = term.trim().to_string();
        if term.is_empty() || self.searching {
            return;
        }
        self.searching = true;
        self.search_error = None;
        cx.notify();
        let generation = self.generation;
        cx.spawn(async move |this, cx| {
            let result = runtime::run(async move { spm::search_market(&term).await }).await;
            this.update(cx, |s, cx| {
                s.searching = false;
                if s.generation == generation {
                    match result {
                        Ok(hits) => {
                            s.search_error = hits.is_empty().then(|| t("skills.market.no_hits").into());
                            s.hits = hits;
                        }
                        Err(e) => {
                            s.hits.clear();
                            s.search_error = Some(e.lines().next().unwrap_or_default().to_string().into());
                        }
                    }
                    s.revision += 1;
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Fetch a market skill and ask the user to review it before anything is installed.
    pub fn review_install(&mut self, market: String, id: String, global: bool, cx: &mut Context<Self>) {
        let label = t_args("skills.busy.review", &[("name", &id)]);
        self.prepare(label, cx, move |dir| async move {
            spm::prepare_install(&dir, &market, &id, global).await
        });
    }

    /// Move a market skill to the newest commit of its repo, after a review.
    pub fn update_latest(&mut self, name: String, cx: &mut Context<Self>) {
        let label = t_args("skills.busy.review", &[("name", &name)]);
        self.prepare(label, cx, move |dir| async move {
            spm::prepare_update_latest(&dir, &name).await
        });
    }

    /// First phase of a market install or update: busy while fetching, then the review dialog.
    fn prepare<F, Fut>(&mut self, label: String, cx: &mut Context<Self>, call: F)
    where
        F: FnOnce(PathBuf) -> Fut + Send + 'static,
        Fut: Future<Output = Result<Prepared, String>> + Send + 'static,
    {
        let Some(dir) = self.project.clone() else {
            return;
        };
        if self.busy.is_some() {
            return;
        }
        self.busy = Some(label.into());
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
                    }
                    s.generation == generation
                })
                .unwrap_or(false);
            match result {
                Err(e) => {
                    tracing::warn!("spm review failed: {e}");
                    cx.update(|cx| toast::error(cx, e));
                }
                Ok(Prepared::Done(msg)) if current => cx.update(|cx| toast::success(cx, msg)),
                Ok(Prepared::Review(review)) if current => {
                    cx.update(|cx| ask_review(cx, this, *review));
                }
                Ok(_) => {}
            }
        })
        .detach();
    }

    /// Second phase: the user approved the review.
    fn apply_review(&mut self, review: Review, cx: &mut Context<Self>) {
        let label = t_args("skills.busy.install", &[("names", &review.name)]);
        self.mutate(label, false, cx, move |dir| async move {
            spm::confirm(&dir, review).await
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

/// The review dialog for a market install or update; approving applies it.
fn ask_review(cx: &mut gpui::App, store: gpui::WeakEntity<SkillsStore>, review: Review) {
    let (title_key, ok_key) = if review.from.is_some() {
        ("skills.review.title_update", "skills.review.update")
    } else if review.global {
        ("skills.review.title_global", "skills.review.install")
    } else {
        ("skills.review.title", "skills.review.install")
    };
    let title = t_args(title_key, &[("name", &review.name), ("market", &review.market)]);
    let options = ConfirmOptions::new(title, review.text()).labels(t(ok_key), t("common.cancel"));
    confirm::ask(cx, options, move |answer, _, cx| {
        if answer == Some(true) {
            store.update(cx, |s, cx| s.apply_review(review, cx)).ok();
        }
    });
}
