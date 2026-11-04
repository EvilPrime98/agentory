//! User preferences. Client-only, persisted in `<config dir>/Agentory/prefs.json`.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::i18n::LanguagePref;
use crate::ui::theme::ThemePref;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Prefs {
    /// Program started in each session. Empty means `claude` from PATH.
    pub claude_command: String,
    /// Offer a git worktree when starting a session in a git project.
    pub ask_worktree: bool,
    /// Submit the prompt (not just type it) when running a doc from the Docs page.
    pub send_doc_prompt: bool,
    /// Toast (in-app and Windows) when a session needs input or finishes.
    pub notifications: bool,
    /// Launch agent sessions in Claude Code's fullscreen renderer so the prompt sits at the bottom.
    pub fullscreen_agents: bool,
    /// Self-build was enabled: the source is cloned and the first build finished.
    pub self_build_enabled: bool,
    pub terminal_font_size: f32,
    pub sidebar_collapsed: bool,
    /// Id of the shell last opened as a project terminal.
    pub last_shell: String,
    /// Interface language. System follows the OS locale.
    pub language: LanguagePref,
    /// Light or dark palette. System follows the OS appearance.
    pub theme: ThemePref,
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            claude_command: String::new(),
            ask_worktree: true,
            send_doc_prompt: false,
            notifications: true,
            fullscreen_agents: true,
            self_build_enabled: false,
            terminal_font_size: 13.0,
            sidebar_collapsed: false,
            last_shell: String::new(),
            language: LanguagePref::System,
            theme: ThemePref::System,
        }
    }
}

pub const MIN_FONT: f32 = 9.0;
pub const MAX_FONT: f32 = 28.0;

pub const CONFIG_DIR_ENV: &str = "AGENTORY_CONFIG_DIR";

pub type Map = serde_json::Map<String, serde_json::Value>;

pub fn config_dir() -> Option<PathBuf> {
    match std::env::var_os(CONFIG_DIR_ENV) {
        Some(dir) if !dir.is_empty() => Some(PathBuf::from(dir)),
        _ => Some(dirs::config_dir()?.join("Agentory")),
    }
}

pub const FILE_NAME: &str = "prefs.json";

fn path() -> Option<PathBuf> {
    Some(config_dir()?.join(FILE_NAME))
}

pub fn read_map() -> std::io::Result<Map> {
    let p = path().ok_or_else(|| std::io::Error::other("no config directory"))?;
    read_map_at(&p)
}

fn read_map_at(p: &Path) -> std::io::Result<Map> {
    match std::fs::read_to_string(p) {
        Ok(s) => match serde_json::from_str(&s) {
            Ok(serde_json::Value::Object(m)) => Ok(m),
            _ => Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("{} is not a JSON object", p.display()),
            )),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Map::new()),
        Err(e) => Err(e),
    }
}

pub fn write_map(map: &Map) -> std::io::Result<()> {
    let p = path().ok_or_else(|| std::io::Error::other("no config directory"))?;
    write_map_at(&p, map)
}

fn write_map_at(p: &Path, map: &Map) -> std::io::Result<()> {
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let json = serde_json::to_string_pretty(map).map_err(std::io::Error::other)?;
    let tmp = p.with_extension(format!("json.tmp{}", std::process::id()));
    std::fs::write(&tmp, json)?;
    std::fs::rename(&tmp, p).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

pub fn to_map(prefs: &Prefs) -> Map {
    match serde_json::to_value(prefs) {
        Ok(serde_json::Value::Object(m)) => m,
        _ => Map::new(),
    }
}

impl Prefs {
    pub fn load() -> Self {
        path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save_changes(before: &Prefs, after: &Prefs) -> Prefs {
        let (old, new) = (to_map(before), to_map(after));
        let mut file = read_map().unwrap_or_else(|_| new.clone());
        for (k, v) in &new {
            if old.get(k) != Some(v) {
                file.insert(k.clone(), v.clone());
            }
        }
        if let Err(e) = write_map(&file) {
            tracing::warn!("could not save prefs: {e}");
            return after.clone();
        }
        serde_json::from_value(serde_json::Value::Object(file)).unwrap_or_else(|_| after.clone())
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
    _watcher: Option<notify::RecommendedWatcher>,
}

impl PrefsStore {
    pub fn load() -> Self {
        Self {
            prefs: Prefs::load(),
            _watcher: None,
        }
    }

    /// Mutate a persisted preference and write it out.
    pub fn update(&mut self, cx: &mut gpui::Context<Self>, change: impl FnOnce(&mut Prefs)) {
        let before = self.prefs.clone();
        change(&mut self.prefs);
        self.prefs.font_clamp();
        self.prefs = Prefs::save_changes(&before, &self.prefs);
        cx.notify();
    }

    fn reload(&mut self, cx: &mut gpui::Context<Self>) {
        let fresh = Prefs::load();
        if fresh != self.prefs {
            self.prefs = fresh;
            cx.notify();
        }
    }

    pub fn watch(&mut self, cx: &mut gpui::Context<Self>) {
        use futures_util::StreamExt as _;
        use notify::Watcher as _;

        let Some(dir) = config_dir() else { return };
        let _ = std::fs::create_dir_all(&dir);
        let (tx, mut rx) = futures_channel::mpsc::unbounded::<()>();
        let watcher = notify::recommended_watcher(move |ev: notify::Result<notify::Event>| {
            let touches_prefs = ev.is_ok_and(|ev| {
                ev.paths
                    .iter()
                    .any(|p| p.file_name().is_some_and(|n| n == FILE_NAME))
            });
            if touches_prefs {
                let _ = tx.unbounded_send(());
            }
        });
        let mut watcher = match watcher {
            Ok(w) => w,
            Err(e) => return tracing::warn!("could not watch prefs: {e}"),
        };
        if let Err(e) = watcher.watch(&dir, notify::RecursiveMode::NonRecursive) {
            return tracing::warn!("could not watch {}: {e}", dir.display());
        }
        self._watcher = Some(watcher);
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
        assert!(!p.send_doc_prompt);
        assert!(!p.self_build_enabled);
        assert_eq!(p.terminal_font_size, 13.0);
        assert_eq!(p.program(), "claude");
    }

    #[test]
    fn language_defaults_to_system_and_round_trips() {
        assert_eq!(Prefs::default().language, LanguagePref::System);
        let p: Prefs = serde_json::from_str(r#"{"language":"spanish"}"#).unwrap();
        assert_eq!(p.language, LanguagePref::Spanish);
    }

    #[test]
    fn theme_defaults_to_system_and_round_trips() {
        assert_eq!(Prefs::default().theme, ThemePref::System);
        let p: Prefs = serde_json::from_str(r#"{"theme":"light"}"#).unwrap();
        assert_eq!(p.theme, ThemePref::Light);
        let json = serde_json::to_string(&Prefs {
            theme: ThemePref::Dark,
            ..Default::default()
        })
        .unwrap();
        assert!(json.contains(r#""theme":"dark""#));
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

#[cfg(test)]
mod persistence_tests {
    use super::*;

    #[test]
    fn atomic_write_replaces_file_and_leaves_no_temp() {
        let dir = std::env::temp_dir().join(format!("agentory-prefs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let p = dir.join(FILE_NAME);
        let mut m = Map::new();
        m.insert("a".into(), 1.into());
        write_map_at(&p, &m).unwrap();
        m.insert("b".into(), 2.into());
        write_map_at(&p, &m).unwrap();
        assert_eq!(read_map_at(&p).unwrap(), m);
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        assert!(read_map_at(&dir.join("missing.json")).unwrap().is_empty());
    }

    #[test]
    fn save_changes_merges_only_changed_keys() {
        let dir = std::env::temp_dir().join(format!("agentory-merge-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        unsafe { std::env::set_var(CONFIG_DIR_ENV, &dir) };

        std::fs::write(dir.join(FILE_NAME), r#"{"theme":"dark","notifications":true}"#).unwrap();
        let before = Prefs::default();
        let after = Prefs {
            notifications: false,
            ..Prefs::default()
        };
        let merged = Prefs::save_changes(&before, &after);
        assert_eq!(merged.theme, ThemePref::Dark);
        assert!(!merged.notifications);
        assert_eq!(Prefs::load(), merged);

        let after2 = Prefs {
            theme: ThemePref::Light,
            ..merged.clone()
        };
        assert_eq!(Prefs::save_changes(&merged, &after2).theme, ThemePref::Light);
        unsafe { std::env::remove_var(CONFIG_DIR_ENV) };
    }
}
