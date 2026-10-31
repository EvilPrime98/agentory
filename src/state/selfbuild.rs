//! State of the "Enable self-build" flow: toolchain checklist, installs, clone, warm build.
//! The work itself lives in `crate::selfbuild`; this store runs it in the background and keeps
//! what the settings page shows.

use std::collections::VecDeque;
use std::future::Future;
use std::time::Duration;

use gpui::{Context, Entity};

use crate::runtime;
use crate::selfbuild::cache;
use crate::selfbuild::clone;
use crate::selfbuild::paths::Paths;
use crate::selfbuild::preflight::{self, Checklist, Tool};
use crate::selfbuild::run::{Event, Log, Stage, channel, stream};
use crate::selfbuild::upstream;
use crate::state::prefs::PrefsStore;

const TAIL_LINES: usize = 8;
const INSTALL_TIMEOUT: Duration = Duration::from_secs(30 * 60);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Work {
    Checking,
    Installing(Tool),
    Cloning,
    Building,
    Cleaning,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Phase {
    Idle,
    Working(Work),
    /// Cloned and built.
    Ready,
    Failed(String),
}

/// What is known about newer official releases.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpstreamState {
    Unknown,
    Checking,
    Checked(upstream::Status),
    Failed(String),
}

pub struct SelfBuildStore {
    pub upstream: UpstreamState,
    pub checklist: Checklist,
    pub phase: Phase,
    /// Most recent output lines of the running step, oldest first.
    pub tail: VecDeque<String>,
    /// Disk used by the shared target dir, once measured.
    pub cache_bytes: Option<u64>,
    pub winget: bool,
    paths: Option<Paths>,
    prefs: Entity<PrefsStore>,
    agent_command: String,
}

impl SelfBuildStore {
    pub fn new(prefs: Entity<PrefsStore>, cx: &mut Context<Self>) -> Self {
        let paths = Paths::system();
        let ready = paths.as_ref().is_some_and(clone::is_ready);
        if let (true, Some(p)) = (ready, paths.as_ref()) {
            // Self-build enabled before the marker existed.
            if let Err(e) = clone::write_marker(p) {
                tracing::warn!("{e}");
            }
        }
        let mut store = Self {
            upstream: UpstreamState::Unknown,
            checklist: Checklist::default(),
            phase: if ready { Phase::Ready } else { Phase::Idle },
            tail: VecDeque::new(),
            cache_bytes: None,
            winget: preflight::winget_available(),
            paths,
            prefs,
            agent_command: String::new(),
        };
        // Once per launch; a failure (offline) only shows up in the settings page.
        if ready {
            store.check_upstream(cx);
        }
        store
    }

    /// Fetch new release tags and compare them with the user's base. Ignored while a step runs.
    pub fn check_upstream(&mut self, cx: &mut Context<Self>) {
        let Some(paths) = self.paths.clone() else { return };
        if self.busy() || self.upstream == UpstreamState::Checking {
            return;
        }
        self.upstream = UpstreamState::Checking;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = runtime::run(async move { upstream::check(&paths.clone_dir()).await }).await;
            this.update(cx, |this, cx| {
                this.upstream = match result {
                    Ok(status) => UpstreamState::Checked(status),
                    Err(e) => {
                        tracing::warn!("upstream check failed: {e}");
                        UpstreamState::Failed(e)
                    }
                };
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The newer release waiting to be merged, if one is known.
    pub fn update_available(&self) -> Option<&str> {
        match &self.upstream {
            UpstreamState::Checked(s) => s.available.as_deref(),
            _ => None,
        }
    }

    pub fn paths(&self) -> Option<&Paths> {
        self.paths.as_ref()
    }

    pub fn busy(&self) -> bool {
        matches!(self.phase, Phase::Working(_))
    }

    /// Everything needed is installed and no step is running.
    pub fn can_enable(&self) -> bool {
        !self.busy() && self.paths.is_some() && self.checklist.ready()
    }

    /// Re-detect the toolchain and measure the build cache. Ignored while a step runs.
    pub fn refresh(&mut self, agent_command: &str, cx: &mut Context<Self>) {
        if self.busy() {
            return;
        }
        self.agent_command = agent_command.to_string();
        self.phase_working(Work::Checking, cx);
        let agent = self.agent_command.clone();
        let paths = self.paths.clone();
        cx.spawn(async move |this, cx| {
            let (list, bytes) = runtime::run(async move {
                let list = preflight::check(&agent).await;
                let bytes = match paths {
                    Some(p) => tokio::task::spawn_blocking(move || cache::target_size(&p)).await.ok(),
                    None => None,
                };
                (list, bytes)
            })
            .await;
            this.update(cx, |this, cx| {
                this.checklist = list;
                this.cache_bytes = bytes;
                this.finish_ok(cx);
            })
            .ok();
        })
        .detach();
    }

    /// Run `winget install` for `tool`. The caller has already shown the command and got a yes.
    pub fn install(&mut self, tool: Tool, cx: &mut Context<Self>) {
        if self.busy() {
            return;
        }
        self.phase_working(Work::Installing(tool), cx);
        let args = tool.install_args();
        self.drive(
            move |mut log| async move {
                let winget = preflight::which("winget")
                    .ok_or("winget was not found; install the App Installer from the Microsoft Store")?;
                let args: Vec<&str> = args.iter().map(String::as_str).collect();
                let exit = stream(
                    &winget.to_string_lossy(),
                    &args,
                    &std::env::temp_dir(),
                    &[],
                    INSTALL_TIMEOUT,
                    &mut log,
                )
                .await?;
                if exit.status.success() {
                    Ok(())
                } else {
                    Err(format!("winget exited with {}", exit.status))
                }
            },
            |this, result, cx| match result {
                // Re-resolve what is installed: PATH changes only reach new processes.
                Ok(()) => {
                    this.phase = Phase::Idle;
                    let agent = this.agent_command.clone();
                    this.refresh(&agent, cx);
                }
                Err(e) => this.fail(e, cx),
            },
            cx,
        );
    }

    /// Clone the source (if needed) and run the first build. The caller has already warned about
    /// the disk use.
    pub fn enable(&mut self, cx: &mut Context<Self>) {
        let Some(paths) = self.paths.clone() else { return };
        if !self.can_enable() {
            return;
        }
        self.phase_working(Work::Cloning, cx);
        self.drive(
            move |mut log| async move { clone::enable(&paths, &mut log).await },
            |this, result, cx| match result {
                Ok(()) => {
                    this.prefs.update(cx, |s, cx| s.update(cx, |p| p.self_build_enabled = true));
                    this.phase = Phase::Idle;
                    let agent = this.agent_command.clone();
                    this.refresh(&agent, cx);
                }
                Err(e) => this.fail(e, cx),
            },
            cx,
        );
    }

    /// Delete the shared target dir.
    pub fn clean_cache(&mut self, cx: &mut Context<Self>) {
        let Some(paths) = self.paths.clone() else { return };
        if self.busy() {
            return;
        }
        self.phase_working(Work::Cleaning, cx);
        cx.spawn(async move |this, cx| {
            let result = runtime::run(async move {
                tokio::task::spawn_blocking(move || cache::clean(&paths))
                    .await
                    .map_err(|e| e.to_string())?
            })
            .await;
            this.update(cx, |this, cx| match result {
                Ok(()) => {
                    this.phase = Phase::Idle;
                    let agent = this.agent_command.clone();
                    this.refresh(&agent, cx);
                }
                Err(e) => this.fail(e, cx),
            })
            .ok();
        })
        .detach();
    }

    // ---- internals ---------------------------------------------------------------------------

    fn phase_working(&mut self, work: Work, cx: &mut Context<Self>) {
        self.phase = Phase::Working(work);
        self.tail.clear();
        cx.notify();
    }

    /// Back to Ready if the build exists, otherwise Idle.
    fn finish_ok(&mut self, cx: &mut Context<Self>) {
        let ready = self.paths.as_ref().is_some_and(clone::is_ready);
        self.phase = if ready { Phase::Ready } else { Phase::Idle };
        cx.notify();
    }

    fn fail(&mut self, message: String, cx: &mut Context<Self>) {
        tracing::warn!("self-build step failed: {message}");
        self.phase = Phase::Failed(message);
        cx.notify();
    }

    fn on_event(&mut self, event: Event, cx: &mut Context<Self>) {
        match event {
            Event::Stage(Stage::Build) => {
                if self.phase == Phase::Working(Work::Cloning) {
                    self.phase = Phase::Working(Work::Building);
                }
            }
            Event::Stage(_) => {}
            Event::Line(line) => {
                if self.tail.len() == TAIL_LINES {
                    self.tail.pop_front();
                }
                self.tail.push_back(line);
            }
        }
        cx.notify();
    }

    /// Run `work` on the Tokio runtime, streaming its log into `tail`, then hand the result to
    /// `done`.
    fn drive<F>(
        &mut self,
        work: impl FnOnce(Log) -> F + Send + 'static,
        done: impl FnOnce(&mut Self, Result<(), String>, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) where
        F: Future<Output = Result<(), String>> + Send + 'static,
    {
        let (tx, mut rx) = channel();
        cx.spawn(async move |this, cx| {
            let job = runtime::run(async move { work(Log::new(tx)).await });
            // The sender lives in the job's `Log`, so the receiver closes when the job ends.
            let pump = async {
                while let Some(event) = rx.recv().await {
                    this.update(cx, |this, cx| this.on_event(event, cx)).ok();
                }
            };
            let (result, ()) = futures_util::future::join(job, pump).await;
            this.update(cx, |this, cx| done(this, result, cx)).ok();
        })
        .detach();
    }
}
