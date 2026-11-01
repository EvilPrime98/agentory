//! Changed-file counts per project, for the sidebar badge. Written by the Git page and by the
//! sidebar's own poll of the selected project.

use std::collections::HashMap;

use gpui::Context;

#[derive(Default)]
pub struct GitStore {
    counts: HashMap<String, usize>,
}

impl GitStore {
    /// Changed files of a project; 0 when unknown or not a repo.
    pub fn count(&self, project_id: &str) -> usize {
        self.counts.get(project_id).copied().unwrap_or(0)
    }

    pub fn set_count(&mut self, project_id: &str, count: usize, cx: &mut Context<Self>) {
        if self.count(project_id) != count {
            self.counts.insert(project_id.to_string(), count);
            cx.notify();
        }
    }
}
