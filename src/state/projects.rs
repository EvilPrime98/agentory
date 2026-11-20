//! The project list shown in the sidebar, persisted in `<config dir>/Agentory/projects.json`.

use std::collections::HashMap;
use std::path::PathBuf;

use gpui::Context;

use crate::backend::scripts::{self, DetectedCommand};
use crate::model::{CustomCommand, GLOBAL_ID, Project, new_id};
use crate::state::prefs;

#[derive(Default)]
pub struct ProjectsStore {
    pub items: Vec<Project>,
    pub selected: Option<String>,
    /// Bumped on every change so observers can memoize.
    pub revision: u64,
    pub detected: HashMap<String, Vec<DetectedCommand>>,
    watcher: Option<notify::RecommendedWatcher>,
}

const FILE_NAME: &str = "projects.json";

fn path() -> Option<PathBuf> {
    Some(prefs::config_dir()?.join(FILE_NAME))
}

pub fn read_saved() -> std::io::Result<Vec<Project>> {
    let p = path().ok_or_else(|| std::io::Error::other("no config directory"))?;
    match std::fs::read_to_string(&p) {
        Ok(s) => serde_json::from_str(&s).map_err(|e| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("{} is not valid: {e}", p.display()),
            )
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(e),
    }
}

pub fn write_saved(projects: &[&Project]) -> std::io::Result<()> {
    let p = path().ok_or_else(|| std::io::Error::other("no config directory"))?;
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let json = serde_json::to_string_pretty(projects).map_err(std::io::Error::other)?;
    let tmp = p.with_extension(format!("json.tmp{}", std::process::id()));
    std::fs::write(&tmp, json)?;
    std::fs::rename(&tmp, &p).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

fn assemble(saved: Vec<Project>) -> Vec<Project> {
    let (globals, rest): (Vec<_>, Vec<_>) = saved.into_iter().partition(Project::is_global);
    let mut global = Project::global();
    if let Some(saved) = globals.into_iter().next() {
        global.skill_args = saved.skill_args;
    }
    let mut items = vec![global];
    items.extend(rest);
    items
}

impl ProjectsStore {
    pub fn load() -> Self {
        let items = assemble(read_saved().unwrap_or_default());
        let selected = items.get(1).unwrap_or(&items[0]).id.clone();
        let mut store = Self {
            selected: Some(selected),
            items,
            ..Self::default()
        };
        let ids: Vec<String> = store.items.iter().map(|p| p.id.clone()).collect();
        for id in ids {
            store.detect(&id);
        }
        store
    }

    pub fn watch(&mut self, cx: &mut Context<Self>) {
        use futures_util::StreamExt as _;
        use notify::Watcher as _;

        let Some(dir) = prefs::config_dir() else {
            return;
        };
        if std::fs::create_dir_all(&dir).is_err() {
            return;
        }
        let (tx, mut rx) = futures_channel::mpsc::unbounded::<()>();
        let watcher = notify::recommended_watcher(move |ev: notify::Result<notify::Event>| {
            if ev.is_ok() {
                let _ = tx.unbounded_send(());
            }
        });
        let mut watcher = match watcher {
            Ok(w) => w,
            Err(e) => return tracing::warn!("could not watch projects: {e}"),
        };
        if let Err(e) = watcher.watch(&dir, notify::RecursiveMode::NonRecursive) {
            return tracing::warn!("could not watch {}: {e}", dir.display());
        }
        self.watcher = Some(watcher);
        cx.spawn(async move |this, cx| {
            while rx.next().await.is_some() {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(150))
                    .await;
                while rx.try_recv().is_ok() {}
                if this.update(cx, |this, cx| this.reload(cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    fn reload(&mut self, cx: &mut Context<Self>) {
        let Ok(saved) = read_saved() else { return };
        let items = assemble(saved);
        if items == self.items {
            return;
        }
        self.items = items;
        let ids: Vec<String> = self.items.iter().map(|p| p.id.clone()).collect();
        self.detected.retain(|id, _| ids.contains(id));
        for id in &ids {
            if !self.detected.contains_key(id) {
                self.detect(id);
            }
        }
        if self.selected_project().is_none() {
            self.selected = self
                .items
                .get(1)
                .or(self.items.first())
                .map(|p| p.id.clone());
        }
        self.revision += 1;
        cx.notify();
    }

    fn detect(&mut self, id: &str) {
        let Some(project) = self.get(id).filter(|p| !p.is_global()) else {
            return;
        };
        let found = scripts::detect(&project.path);
        self.detected.insert(id.to_string(), found);
    }

    fn save(&self) {
        let kept: Vec<&Project> = self
            .items
            .iter()
            .filter(|p| !p.is_global() || !p.skill_args.is_empty())
            .collect();
        if let Err(e) = write_saved(&kept) {
            tracing::warn!("could not save projects: {e}");
        }
    }

    pub fn get(&self, id: &str) -> Option<&Project> {
        self.items.iter().find(|p| p.id == id)
    }

    pub fn path_of(&self, id: &str) -> Option<PathBuf> {
        self.get(id).map(|p| p.path.clone())
    }

    pub fn selected_project(&self) -> Option<&Project> {
        self.selected.as_deref().and_then(|id| self.get(id))
    }

    /// Add a folder (no-op for one already listed) and select it. Returns the project id.
    pub fn add(&mut self, path: PathBuf, cx: &mut Context<Self>) -> String {
        let id = match self.items.iter().find(|p| !p.is_global() && p.path == path) {
            Some(existing) => existing.id.clone(),
            None => {
                let project = Project::from_path(path);
                let id = project.id.clone();
                self.items.push(project);
                self.detect(&id);
                self.save();
                id
            }
        };
        self.selected = Some(id.clone());
        self.revision += 1;
        cx.notify();
        id
    }

    pub fn remove(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.get(id).is_none_or(Project::is_global) {
            return;
        }
        self.items.retain(|p| p.id != id);
        self.detected.remove(id);
        if self.selected.as_deref() == Some(id) {
            self.selected = self.items.first().map(|p| p.id.clone());
        }
        self.save();
        self.revision += 1;
        cx.notify();
    }

    pub fn move_project(&mut self, id: &str, target_id: &str, after: bool, cx: &mut Context<Self>) {
        if id == GLOBAL_ID || (target_id == GLOBAL_ID && !after) {
            return;
        }
        let (Some(from), Some(target)) = (
            self.items.iter().position(|p| p.id == id),
            self.items.iter().position(|p| p.id == target_id),
        ) else {
            return;
        };
        let to = target + usize::from(after);
        if to == from || to == from + 1 {
            return;
        }
        let project = self.items.remove(from);
        self.items
            .insert(if to > from { to - 1 } else { to }, project);
        self.save();
        self.revision += 1;
        cx.notify();
    }

    /// Remember (or with `None` forget) whether new sessions of a project use a worktree.
    pub fn set_use_worktree(&mut self, id: &str, value: Option<bool>, cx: &mut Context<Self>) {
        let Some(project) = self.items.iter_mut().find(|p| p.id == id) else {
            return;
        };
        if project.use_worktree == value {
            return;
        }
        project.use_worktree = value;
        self.save();
        self.revision += 1;
        cx.notify();
    }

    pub fn set_skill_args(&mut self, id: &str, skill: &str, args: &str, cx: &mut Context<Self>) {
        let Some(project) = self.items.iter_mut().find(|p| p.id == id) else {
            return;
        };
        let args = args.trim();
        let changed = if args.is_empty() {
            project.skill_args.remove(skill).is_some()
        } else {
            project
                .skill_args
                .insert(skill.to_string(), args.to_string())
                .as_deref()
                != Some(args)
        };
        if changed {
            self.save();
            self.revision += 1;
            cx.notify();
        }
    }

    pub fn set_default_shell(&mut self, id: &str, shell: Option<String>, cx: &mut Context<Self>) {
        let Some(project) = self.items.iter_mut().find(|p| p.id == id) else {
            return;
        };
        if project.default_shell == shell {
            return;
        }
        project.default_shell = shell;
        self.save();
        self.revision += 1;
        cx.notify();
    }

    pub fn save_command(&mut self, id: &str, mut command: CustomCommand, cx: &mut Context<Self>) {
        let Some(project) = self.items.iter_mut().find(|p| p.id == id) else {
            return;
        };
        match project.commands.iter_mut().find(|c| c.id == command.id) {
            Some(existing) => *existing = command,
            None => {
                command.id = new_id("c");
                project.commands.push(command);
            }
        }
        self.save();
        self.revision += 1;
        cx.notify();
    }

    pub fn delete_command(&mut self, id: &str, command_id: &str, cx: &mut Context<Self>) {
        let Some(project) = self.items.iter_mut().find(|p| p.id == id) else {
            return;
        };
        let before = project.commands.len();
        project.commands.retain(|c| c.id != command_id);
        if project.commands.len() != before {
            self.save();
            self.revision += 1;
            cx.notify();
        }
    }

    pub fn select(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.selected.as_deref() != Some(id) && self.get(id).is_some() {
            self.selected = Some(id.to_string());
            self.detect(id);
            self.revision += 1;
            cx.notify();
        }
    }
}
