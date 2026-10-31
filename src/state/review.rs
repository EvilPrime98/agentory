//! Reviewing and applying a Modify session's change. One apply runs at a time (they share the
//! build cache); its stage, log and result are kept here for the review page and status pill.
//! The sequence itself lives in `crate::selfbuild::apply`.

use futures_util::future::join;
use gpui::Context;

use crate::backend::git::Worktree;
use crate::i18n::{t, t_args};
use crate::runtime;
use crate::selfbuild::apply::{self, Applied, Staged};
use crate::selfbuild::paths::Paths;
use crate::selfbuild::run::{Event, Outcome, Stage, channel};
use crate::state::Stores;
use crate::ui::toast;

/// Log lines kept for the log view.
const MAX_LOG_LINES: usize = 4000;
/// Log lines handed to the agent when sending a failure back.
const SEND_LINES: usize = 80;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Failure {
    Build,
    Tests,
    Smoke,
    /// Git or file trouble before or after the build; `user` was not changed.
    Other(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunPhase {
    Running,
    /// Merging `user` into the change stopped on these files; the session was asked to resolve.
    Conflict(Vec<String>),
    Failed(Failure),
    /// Built, gated and merged into `user`; waiting for the restart.
    Ready(Staged),
}

pub struct Run {
    pub session: String,
    pub phase: RunPhase,
    /// The stage that last started; on a failure, the one that failed.
    pub stage: Option<Stage>,
    pub log: Vec<String>,
}

#[derive(Default)]
pub struct ReviewStore {
    pub run: Option<Run>,
}

/// What the status pill shows for one stage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StageState {
    Pending,
    Active,
    Done,
    Failed,
}

pub const STAGES: [Stage; 4] = [Stage::Merge, Stage::Build, Stage::Gate, Stage::Ready];

fn index(stage: Stage) -> usize {
    STAGES.iter().position(|s| *s == stage).unwrap_or(0)
}

impl Run {
    fn new(session: &str) -> Run {
        Run {
            session: session.to_string(),
            phase: RunPhase::Running,
            stage: None,
            log: Vec::new(),
        }
    }

    pub fn stage_state(&self, stage: Stage) -> StageState {
        let me = index(stage);
        match (&self.phase, self.stage) {
            (RunPhase::Ready(_), _) => StageState::Done,
            (_, None) => StageState::Pending,
            (phase, Some(cur)) => {
                let cur = index(cur);
                if me < cur {
                    StageState::Done
                } else if me > cur {
                    StageState::Pending
                } else if matches!(phase, RunPhase::Running) {
                    StageState::Active
                } else {
                    StageState::Failed
                }
            }
        }
    }
}

/// What the agent is told when the merge with `user` conflicts.
pub fn conflict_prompt(files: &[String]) -> String {
    format!(
        "Merging the latest user branch into this worktree stopped with conflicts in: {}. \
         Resolve every conflict and remove all conflict markers, keeping the intent of both sides. \
         Do not weaken or delete tests. Run cargo build and cargo test, commit, then tell me it is ready to apply again.",
        files.join(", ")
    )
}

/// What the agent is told when a build, the tests or the smoke launch failed.
pub fn failure_prompt(log: &[String]) -> String {
    let tail = &log[log.len().saturating_sub(SEND_LINES)..];
    format!(
        "Applying this change failed. Fix the cause without weakening any tests or checks, then tell me it is ready to apply again. End of the log:\n{}",
        tail.join("\n")
    )
}

impl ReviewStore {
    pub fn running(&self) -> bool {
        self.run
            .as_ref()
            .is_some_and(|r| r.phase == RunPhase::Running)
    }

    /// The run for `session`, if the last one was for it.
    pub fn for_session(&self, session: &str) -> Option<&Run> {
        self.run.as_ref().filter(|r| r.session == session)
    }

    /// Merge, build, gate and stage the session's change. Ignored while another apply runs.
    pub fn apply(&mut self, session_id: &str, cx: &mut Context<Self>) {
        if self.running() {
            return;
        }
        let stores = cx.global::<Stores>().clone();
        let wt = stores
            .sessions
            .read(cx)
            .get(session_id)
            .and_then(|s| s.worktree.clone());
        let paths = stores.selfbuild.read(cx).paths().cloned();
        let (Some(wt), Some(paths)) = (wt, paths) else {
            toast::error(cx, t("review.unavailable"));
            return;
        };
        self.run = Some(Run::new(session_id));
        cx.notify();

        let (tx, mut rx) = channel();
        let sid = session_id.to_string();
        cx.spawn(async move |this, cx| {
            let (p, w) = (paths.clone(), wt.clone());
            let job = runtime::run(async move { apply::apply(&p, &w, tx).await });
            // The senders live inside the job, so the receiver closes when it ends.
            let pump = async {
                while let Some(event) = rx.recv().await {
                    this.update(cx, |this, cx| this.on_event(event, cx)).ok();
                }
            };
            let (result, ()) = join(job, pump).await;
            this.update(cx, |this, cx| this.finish(sid, paths, wt, result, cx))
                .ok();
        })
        .detach();
    }

    fn on_event(&mut self, event: Event, cx: &mut Context<Self>) {
        let Some(run) = self.run.as_mut() else { return };
        match event {
            Event::Stage(stage) => run.stage = Some(stage),
            Event::Line(line) => {
                if run.log.len() == MAX_LOG_LINES {
                    run.log.remove(0);
                }
                run.log.push(line);
            }
        }
        cx.notify();
    }

    fn finish(
        &mut self,
        session: String,
        paths: Paths,
        wt: Worktree,
        result: Applied,
        cx: &mut Context<Self>,
    ) {
        let stores = cx.global::<Stores>().clone();
        let Some(run) = self.run.as_mut().filter(|r| r.session == session) else {
            return;
        };
        run.phase = match result {
            Applied::Conflict(files) => {
                let prompt = conflict_prompt(&files);
                stores
                    .sessions
                    .update(cx, |s, cx| s.prefill(&session, prompt, cx));
                RunPhase::Conflict(files)
            }
            Applied::Failed(Outcome::BuildFailed(_)) => RunPhase::Failed(Failure::Build),
            Applied::Failed(Outcome::TestsFailed(_)) => RunPhase::Failed(Failure::Tests),
            Applied::Failed(Outcome::SmokeFailed(_)) => RunPhase::Failed(Failure::Smoke),
            Applied::Failed(Outcome::Passed(_)) => {
                RunPhase::Failed(Failure::Other("unexpected pass".into()))
            }
            Applied::Error(e) => {
                run.log.push(e.clone());
                RunPhase::Failed(Failure::Other(e))
            }
            Applied::Ready(staged) => {
                // `user` holds the change now, so the worktree and its session are done.
                stores.sessions.update(cx, |s, cx| s.close(&session, cx));
                Self::remove_worktree(paths, wt, cx);
                // `user` moved (an upstream merge changes the base), so refresh the comparison.
                stores.selfbuild.update(cx, |s, cx| s.check_upstream(cx));
                RunPhase::Ready(staged)
            }
        };
        cx.notify();
    }

    /// Type the end of the failure log into the session's input, for the agent to fix.
    pub fn send_log(&mut self, session: &str, cx: &mut Context<Self>) {
        let Some(run) = self.for_session(session) else {
            return;
        };
        let prompt = failure_prompt(&run.log);
        let stores = cx.global::<Stores>().clone();
        stores
            .sessions
            .update(cx, |s, cx| s.prefill(session, prompt, cx));
        stores.sessions.update(cx, |s, cx| s.select(session, cx));
    }

    /// Close the session and delete its worktree and branch. Whatever it held is lost.
    pub fn discard(&mut self, session: &str, cx: &mut Context<Self>) {
        if self.running() {
            return;
        }
        let stores = cx.global::<Stores>().clone();
        let wt = stores
            .sessions
            .read(cx)
            .get(session)
            .and_then(|s| s.worktree.clone());
        let paths = stores.selfbuild.read(cx).paths().cloned();
        stores.sessions.update(cx, |s, cx| s.close(session, cx));
        if self.run.as_ref().is_some_and(|r| r.session == session) {
            self.run = None;
        }
        if let (Some(wt), Some(paths)) = (wt, paths) {
            Self::remove_worktree(paths, wt, cx);
        }
        cx.notify();
    }

    fn remove_worktree(paths: Paths, wt: Worktree, cx: &mut Context<Self>) {
        cx.spawn(async move |_, cx| {
            let branch = wt.branch.clone();
            let result = runtime::run(async move { apply::discard(&paths, &wt).await }).await;
            if let Err(e) = result {
                tracing::warn!("could not remove worktree {branch}: {e}");
                cx.update(|cx| {
                    toast::error(
                        cx,
                        t_args(
                            "review.cleanup_failed",
                            &[("branch", &branch), ("error", &e)],
                        ),
                    )
                });
            }
        })
        .detach();
    }

    /// Hand over to the helper, which swaps the exe once this process is gone, and quit.
    pub fn restart(&mut self, cx: &mut Context<Self>) {
        let Some(RunPhase::Ready(staged)) = self.run.as_ref().map(|r| r.phase.clone()) else {
            return;
        };
        match apply::start_swap(&staged) {
            // Quitting kills the sessions' children (see `AppRoot`).
            Ok(()) => cx.quit(),
            Err(e) => toast::error(cx, t_args("review.restart_failed", &[("error", &e)])),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run_at(phase: RunPhase, stage: Option<Stage>) -> Run {
        Run {
            session: "m1".into(),
            phase,
            stage,
            log: vec![],
        }
    }

    fn pill(run: &Run) -> Vec<StageState> {
        STAGES.iter().map(|s| run.stage_state(*s)).collect()
    }

    #[test]
    fn pill_follows_the_running_stage() {
        use StageState::*;
        assert_eq!(pill(&run_at(RunPhase::Running, None)), [Pending; 4]);
        assert_eq!(
            pill(&run_at(RunPhase::Running, Some(Stage::Build))),
            [Done, Active, Pending, Pending]
        );
        assert_eq!(
            pill(&run_at(RunPhase::Running, Some(Stage::Gate))),
            [Done, Done, Active, Pending]
        );
    }

    #[test]
    fn pill_marks_the_failed_stage() {
        use StageState::*;
        let failed = run_at(RunPhase::Failed(Failure::Tests), Some(Stage::Gate));
        assert_eq!(pill(&failed), [Done, Done, Failed, Pending]);
        let conflict = run_at(RunPhase::Conflict(vec!["a".into()]), Some(Stage::Merge));
        assert_eq!(pill(&conflict), [Failed, Pending, Pending, Pending]);
    }

    #[test]
    fn pill_is_all_done_when_ready() {
        let staged = Staged {
            exe: "x".into(),
            build_id: "b".into(),
        };
        let ready = run_at(RunPhase::Ready(staged), Some(Stage::Ready));
        assert_eq!(pill(&ready), [StageState::Done; 4]);
    }

    #[test]
    fn prompts_name_the_files_and_end_of_the_log() {
        let p = conflict_prompt(&["a.rs".into(), "b.rs".into()]);
        assert!(p.contains("a.rs, b.rs") && !p.contains('\r'));
        let log: Vec<String> = (0..200).map(|i| format!("line {i}")).collect();
        let p = failure_prompt(&log);
        assert!(p.contains("line 199") && p.contains("line 120") && !p.contains("line 119\n"));
        assert!(!p.contains('\r'), "a CR would submit the prompt");
    }

    #[test]
    fn only_a_running_run_blocks_another() {
        let mut s = ReviewStore::default();
        assert!(!s.running());
        s.run = Some(run_at(RunPhase::Running, None));
        assert!(s.running());
        s.run = Some(run_at(RunPhase::Failed(Failure::Build), Some(Stage::Build)));
        assert!(!s.running());
        assert!(s.for_session("m1").is_some() && s.for_session("other").is_none());
    }
}
