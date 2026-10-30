//! User preferences. Client-only, persisted in `<config dir>/Agentory/prefs.json`.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Prefs {
    /// Program started in each session. Empty means `claude` from PATH.
    pub claude_command: String,
    /// Offer a git worktree when starting a session in a git project.
    pub ask_worktree: bool,
    /// Toast (in-app and Windows) when a session needs input or finishes.
    pub notifications: bool,
    /// Launch agent sessions in Claude Code's fullscreen renderer so the prompt sits at the bottom.
    pub fullscreen_agents: bool,
    pub terminal_font_size: f32,
    pub sidebar_collapsed: bool,
    pub skills_drawer_open: bool,
    /// Id of the shell last opened as a project terminal.
    pub last_shell: String,
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            claude_command: String::new(),
            ask_worktree: true,
            notifications: true,
            fullscreen_agents: true,
            terminal_font_size: 13.0,
            sidebar_collapsed: false,
            skills_drawer_open: false,
            last_shell: String::new(),
        }
    }
}

pub const MIN_FONT: f32 = 9.0;
pub const MAX_FONT: f32 = 28.0;

fn path() -> Option<PathBuf> {
    Some(dirs::config_dir()?.join("Agentory").join("prefs.json"))
}

impl Prefs {
    pub fn load() -> Self {
        path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    /// Best effort: losing a preference is not worth an error dialog.
    pub fn save(&self) {
        let Some(p) = path() else { return };
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(json) = serde_json::to_string_pretty(self) {
            if let Err(e) = std::fs::write(&p, json) {
                tracing::warn!("could not save prefs to {}: {e}", p.display());
            }
        }
    }

    /// The command to start, `claude` unless overridden.
    pub fn program(&self) -> &str {
        let c = self.claude_command.trim();
        if c.is_empty() { "claude" } else { c }
    }
}

/// Entity wrapper around the persisted [`Prefs`].
pub struct PrefsStore {
    pub prefs: Prefs,
}

impl PrefsStore {
    pub fn load() -> Self {
        Self {
            prefs: Prefs::load(),
        }
    }

    /// Mutate a persisted preference and write it out.
    pub fn update(&mut self, cx: &mut gpui::Context<Self>, change: impl FnOnce(&mut Prefs)) {
        change(&mut self.prefs);
        self.prefs.font_clamp();
        self.prefs.save();
        cx.notify();
    }
}

impl Prefs {
    fn font_clamp(&mut self) {
        self.terminal_font_size = self.terminal_font_size.clamp(MIN_FONT, MAX_FONT);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partial_files_fall_back_to_defaults() {
        let p: Prefs = serde_json::from_str(r#"{"notifications":false}"#).unwrap();
        assert!(!p.notifications);
        assert!(p.ask_worktree);
        assert_eq!(p.terminal_font_size, 13.0);
        assert_eq!(p.program(), "claude");
    }

    #[test]
    fn font_size_is_clamped() {
        let mut p = Prefs {
            terminal_font_size: 99.0,
            ..Default::default()
        };
        p.font_clamp();
        assert_eq!(p.terminal_font_size, MAX_FONT);
    }
}
