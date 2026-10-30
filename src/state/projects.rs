//! The project list shown in the sidebar, persisted in `<config dir>/Agentory/projects.json`.

use std::path::PathBuf;

use gpui::Context;

use crate::model::Project;

#[derive(Default)]
pub struct ProjectsStore {
    pub items: Vec<Project>,
    pub selected: Option<String>,
    /// Bumped on every change so observers can memoize.
    pub revision: u64,
}

fn path() -> Option<PathBuf> {
    Some(dirs::config_dir()?.join("Agentory").join("projects.json"))
}

impl ProjectsStore {
    pub fn load() -> Self {
        let items: Vec<Project> = path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        Self {
            selected: items.first().map(|p| p.id.clone()),
            items,
            revision: 0,
        }
    }

    fn save(&self) {
        let Some(p) = path() else { return };
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(json) = serde_json::to_string_pretty(&self.items) {
            if let Err(e) = std::fs::write(&p, json) {
                tracing::warn!("could not save projects to {}: {e}", p.display());
            }
        }
    }

    pub fn get(&self, id: &str) -> Option<&Project> {
        self.items.iter().find(|p| p.id == id)
    }

    pub fn selected_project(&self) -> Option<&Project> {
        self.selected.as_deref().and_then(|id| self.get(id))
    }

    /// Add a folder (no-op for one already listed) and select it. Returns the project id.
    pub fn add(&mut self, path: PathBuf, cx: &mut Context<Self>) -> String {
        let id = match self.items.iter().find(|p| p.path == path) {
            Some(existing) => existing.id.clone(),
            None => {
                let project = Project::from_path(path);
                let id = project.id.clone();
                self.items.push(project);
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
        self.items.retain(|p| p.id != id);
        if self.selected.as_deref() == Some(id) {
            self.selected = self.items.first().map(|p| p.id.clone());
        }
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

    pub fn select(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.selected.as_deref() != Some(id) && self.get(id).is_some() {
            self.selected = Some(id.to_string());
            self.revision += 1;
            cx.notify();
        }
    }
}
