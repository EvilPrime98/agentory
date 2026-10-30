//! Claude Code sessions: one PTY-backed terminal each, grouped by project, with a status fed by
//! the hooks (see `platform::status`).

use std::path::PathBuf;
use std::time::Duration;

use gpui::{App, AppContext as _, Context, Entity, Subscription};

use crate::backend::git::{self, Worktree};
use crate::backend::spm;
use crate::model::{Project, SessionStatus, new_id};
use crate::platform::pty::{self, SpawnSpec};
use crate::platform::{notify, status};
use crate::runtime;
use crate::state::Stores;
use crate::state::prefs::Prefs;
use crate::ui::confirm::{self, ConfirmOptions};
use crate::ui::terminal::{Terminal, TerminalExited};
use crate::ui::toast;

const POLL_INTERVAL: Duration = Duration::from_millis(500);

pub struct Session {
    pub id: String,
    pub project_id: String,
    pub name: String,
    pub cwd: PathBuf,
    pub worktree: Option<Worktree>,
    pub status: SessionStatus,
    pub terminal: Entity<Terminal>,
    _exit_sub: Subscription,
}

pub struct SessionsStore {
    pub items: Vec<Session>,
    pub active: Option<String>,
    /// Bumped on every structural change so observers can memoize.
    pub revision: u64,
    /// Whether the app window is in the foreground (toasts go to Windows when it is not).
    pub app_active: bool,
}

impl SessionsStore {
    pub fn new(cx: &mut Context<Self>) -> Self {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(POLL_INTERVAL).await;
                if this.update(cx, |s, cx| s.poll(cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
        Self {
            items: Vec::new(),
            active: None,
            revision: 0,
            app_active: true,
        }
    }

    pub fn get(&self, id: &str) -> Option<&Session> {
        self.items.iter().find(|s| s.id == id)
    }

    pub fn active_session(&self) -> Option<&Session> {
        self.active.as_deref().and_then(|id| self.get(id))
    }

    pub fn for_project<'a>(&'a self, project_id: &'a str) -> impl Iterator<Item = &'a Session> {
        self.items
            .iter()
            .filter(move |s| s.project_id == project_id)
    }

    /// Sessions whose process is still running.
    pub fn running(&self) -> usize {
        self.items
            .iter()
            .filter(|s| s.status != SessionStatus::Exited)
            .count()
    }

    pub fn select(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.active.as_deref() != Some(id) && self.get(id).is_some() {
            self.active = Some(id.to_string());
            self.revision += 1;
            cx.notify();
        }
    }

    /// Start a session. `id` is shared with the worktree (if any) so both are easy to correlate.
    pub fn start(
        &mut self,
        project: &Project,
        id: String,
        worktree: Option<Worktree>,
        prefs: &Prefs,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let cwd = worktree
            .as_ref()
            .map_or_else(|| project.path.clone(), |w| w.path.clone());
        let (program, args) = if std::env::var_os("AGENTORY_SHELL").is_some() {
            // Dev aid: a plain shell instead of Claude.
            ("powershell.exe".to_string(), Vec::new())
        } else {
            let program = pty::resolve_program(prefs.program()).ok_or_else(|| {
                format!(
                    "`{}` was not found on PATH. Install Claude Code or set the command in Settings.",
                    prefs.program()
                )
            })?;
            let settings = status::write_settings(&id)
                .map_err(|e| format!("could not write the hook settings: {e}"))?;
            (
                program.to_string_lossy().into_owned(),
                vec![
                    "--settings".to_string(),
                    settings.to_string_lossy().into_owned(),
                ],
            )
        };
        let (rows, cols) = (30, 100);
        let spawned = pty::spawn(SpawnSpec {
            program,
            args,
            cwd: cwd.clone(),
            env: vec![("AGENTORY_SESSION".into(), id.clone())],
            rows,
            cols,
        })
        .map_err(|e| format!("{e:#}"))?;
        let font_size = prefs.terminal_font_size;
        let terminal = cx.new(|cx| Terminal::new(spawned, rows, cols, font_size, cx));

        let n = self.for_project(&project.id).count() + 1;
        let name = match &worktree {
            Some(_) => format!("{} #{n} (worktree)", project.name),
            None => format!("{} #{n}", project.name),
        };
        let exit_id = id.clone();
        let exit_sub = cx.subscribe(&terminal, move |this, _, _: &TerminalExited, cx| {
            this.set_status(&exit_id, SessionStatus::Exited, cx);
        });
        self.items.push(Session {
            id: id.clone(),
            project_id: project.id.clone(),
            name,
            cwd,
            worktree,
            status: SessionStatus::Starting,
            terminal,
            _exit_sub: exit_sub,
        });
        self.active = Some(id);
        self.revision += 1;
        cx.notify();
        Ok(())
    }

    pub fn close(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(ix) = self.items.iter().position(|s| s.id == id) else {
            return;
        };
        let session = self.items.remove(ix);
        session.terminal.update(cx, |t, _| t.kill());
        status::cleanup(id);
        if self.active.as_deref() == Some(id) {
            self.active = self
                .items
                .get(ix.min(self.items.len().saturating_sub(1)))
                .map(|s| s.id.clone());
        }
        self.revision += 1;
        cx.notify();
    }

    /// Close every session of a project (the project was removed).
    pub fn close_project(&mut self, project_id: &str, cx: &mut Context<Self>) {
        let ids: Vec<String> = self.for_project(project_id).map(|s| s.id.clone()).collect();
        for id in ids {
            self.close(&id, cx);
        }
    }

    pub fn kill_all(&mut self, cx: &mut Context<Self>) {
        for s in &self.items {
            s.terminal.update(cx, |t, _| t.kill());
            status::cleanup(&s.id);
        }
    }

    fn set_status(&mut self, id: &str, new: SessionStatus, cx: &mut Context<Self>) {
        let Some(s) = self.items.iter_mut().find(|s| s.id == id) else {
            return;
        };
        let old = s.status;
        if old == new {
            return;
        }
        s.status = new;
        let name = s.name.clone();
        cx.notify();
        self.maybe_notify(id, &name, old, new, cx);
    }

    /// Toast when a session needs input or finishes a turn and the user is not looking at it.
    fn maybe_notify(
        &self,
        id: &str,
        name: &str,
        old: SessionStatus,
        new: SessionStatus,
        cx: &mut Context<Self>,
    ) {
        let message = match (old, new) {
            (_, SessionStatus::NeedsInput) => "needs your input",
            (SessionStatus::Working, SessionStatus::Idle) => "finished",
            _ => return,
        };
        let enabled = cx
            .try_global::<Stores>()
            .is_some_and(|s| s.prefs.read(cx).prefs.notifications);
        let watching = self.app_active && self.active.as_deref() == Some(id);
        if !enabled || watching {
            return;
        }
        toast::success(cx, format!("{name} {message}"));
        if !self.app_active {
            let (title, body) = ("Agentory".to_string(), format!("{name} {message}"));
            cx.spawn(async move |_, _| runtime::run(notify::toast(title, body)).await)
                .detach();
        }
    }

    /// Every tick: detect exited children and apply the newest hook event.
    fn poll(&mut self, cx: &mut Context<Self>) {
        let live: Vec<(String, Entity<Terminal>)> = self
            .items
            .iter()
            .filter(|s| s.status != SessionStatus::Exited)
            .map(|s| (s.id.clone(), s.terminal.clone()))
            .collect();
        for (id, terminal) in live {
            terminal.update(cx, |t, cx| t.check_exit(cx));
            let exited = self
                .get(&id)
                .is_some_and(|s| s.status == SessionStatus::Exited);
            if exited {
                continue;
            }
            if let Some(event) = status::read_event(&id) {
                self.set_status(&id, event.status(), cx);
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Starting a session from the UI
// ---------------------------------------------------------------------------------------------

/// "New session" for a project. Git projects use the remembered worktree choice; without one they
/// are asked (when `ask_worktree` is on) and the answer can be remembered for the project.
pub fn begin_new(project_id: &str, cx: &mut App) {
    let stores = cx.global::<Stores>().clone();
    let Some(project) = stores.projects.read(cx).get(project_id).cloned() else {
        return;
    };
    if !project.is_git_repo() {
        launch(stores, project, false, cx);
        return;
    }
    if let Some(worktree) = project.use_worktree {
        launch(stores, project, worktree, cx);
        return;
    }
    if !stores.prefs.read(cx).prefs.ask_worktree {
        launch(stores, project, false, cx);
        return;
    }
    let options = ConfirmOptions::new(
        "New session",
        format!(
            "Run a session for \"{}\" in an isolated git worktree (new branch), or directly in the project folder?",
            project.name
        ),
    )
    .labels("Worktree", "Project folder")
    .checkbox("Remember for this project", true);
    confirm::ask(cx, options, move |answer, remember, cx| {
        let Some(worktree) = answer else { return };
        if remember {
            stores
                .projects
                .update(cx, |p, cx| p.set_use_worktree(&project.id, Some(worktree), cx));
        }
        launch(stores, project, worktree, cx);
    });
}

/// Start one session in an explicit mode, ignoring and keeping the remembered choice.
pub fn begin_new_with(project_id: &str, worktree: bool, cx: &mut App) {
    let stores = cx.global::<Stores>().clone();
    let Some(project) = stores.projects.read(cx).get(project_id).cloned() else {
        return;
    };
    let worktree = worktree && project.is_git_repo();
    launch(stores, project, worktree, cx);
}

fn launch(stores: Stores, project: Project, worktree: bool, cx: &mut App) {
    let id = new_id("s");
    if !worktree {
        spawn_session(&stores, &project, id, None, cx);
        return;
    }
    toast::success(cx, "Creating the worktree…");
    cx.spawn(async move |cx| {
        let (p, sid) = (project.clone(), id.clone());
        let result = runtime::run(async move {
            let wt = git::add_worktree(&p.path, &p.name, &sid).await?;
            // `.claude/skills` is not carried over by git when it is untracked.
            if p.path.join("spm.json").exists() {
                if let Err(e) = spm::install(&wt.path, &[]).await {
                    tracing::warn!("spm install in the worktree failed: {e}");
                }
            }
            Ok::<_, String>(wt)
        })
        .await;
        cx.update(|cx| match result {
            Ok(wt) => spawn_session(&stores, &project, id, Some(wt), cx),
            Err(e) => toast::error(cx, format!("Could not create the worktree: {e}")),
        });
    })
    .detach();
}

fn spawn_session(
    stores: &Stores,
    project: &Project,
    id: String,
    worktree: Option<Worktree>,
    cx: &mut App,
) {
    let prefs = stores.prefs.read(cx).prefs.clone();
    let result = stores
        .sessions
        .update(cx, |s, cx| s.start(project, id, worktree, &prefs, cx));
    if let Err(e) = result {
        tracing::error!("could not start a session: {e}");
        toast::error(cx, e);
    }
}
