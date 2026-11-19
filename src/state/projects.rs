//! The project list shown in the sidebar, persisted in `<config dir>/Agentory/projects.json`.

use std::collections::HashMap;
use std::path::PathBuf;

use gpui::Context;

use crate::backend::scripts::{self, DetectedCommand};
use crate::model::{CustomCommand, GLOBAL_ID, Project, new_id};

#[derive(Default)]
pub struct ProjectsStore {
    pub items: Vec<Project>,
    pub selected: Option<String>,
    /// Bumped on every change so observers can memoize.
    pub revision: u64,
    pub detected: HashMap<String, Vec<DetectedCommand>>,
}

fn path() -> Option<PathBuf> {
    Some(dirs::config_dir()?.join("Agentory").join("projects.json"))
}

impl ProjectsStore {
    pub fn load() -> Self {
        let saved: Vec<Project> = path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        let (globals, rest): (Vec<_>, Vec<_>) = saved.into_iter().partition(Project::is_global);
        let mut global = Project::global();
        if let Some(saved) = globals.into_iter().next() {
            global.skill_args = saved.skill_args;
        }
        let selected = rest.first().unwrap_or(&global).id.clone();
        let mut items = vec![global];
        items.extend(rest);
        let mut store = Self {
            selected: Some(selected),
            items,
            revision: 0,
            detected: HashMap::new(),
        };
        let ids: Vec<String> = store.items.iter().map(|p| p.id.clone()).collect();
        for id in ids {
            store.detect(&id);
        }
        store
    }

    fn detect(&mut self, id: &str) {
        let Some(project) = self.get(id).filter(|p| !p.is_global()) else {
            return;
        };
        let found = scripts::detect(&project.path);
        self.detected.insert(id.to_string(), found);
    }

    fn save(&self) {
        let Some(p) = path() else { return };
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let kept: Vec<&Project> = self
            .items
            .iter()
            .filter(|p| !p.is_global() || !p.skill_args.is_empty())
            .collect();
        if let Ok(json) = serde_json::to_string_pretty(&kept) {
            if let Err(e) = std::fs::write(&p, json) {
                tracing::warn!("could not save projects to {}: {e}", p.display());
            }
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
