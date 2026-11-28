//! App-level keyboard shortcuts: the rebindable actions and the one validator the Settings tab
//! and the `agentory settings` CLI share.

use std::fmt;

use gpui::Keystroke;

use crate::state::prefs::Prefs;

pub struct Action {
    pub key: &'static str,
    pub field: &'static str,
    pub label_key: &'static str,
    pub default: &'static str,
}

pub const ACTIONS: [Action; 8] = [
    Action {
        key: "shortcut-toggle-fullscreen",
        field: "shortcutToggleFullscreen",
        label_key: "settings.shortcuts.toggle_fullscreen",
        default: "f11",
    },
    Action {
        key: "shortcut-toggle-sidebar",
        field: "shortcutToggleSidebar",
        label_key: "settings.shortcuts.toggle_sidebar",
        default: "ctrl-alt-b",
    },
    Action {
        key: "shortcut-open-skills",
        field: "shortcutOpenSkills",
        label_key: "settings.shortcuts.open_skills",
        default: "ctrl-alt-s",
    },
    Action {
        key: "shortcut-open-terminal",
        field: "shortcutOpenTerminal",
        label_key: "settings.shortcuts.open_terminal",
        default: "ctrl-alt-t",
    },
    Action {
        key: "shortcut-toggle-microphone",
        field: "shortcutToggleMicrophone",
        label_key: "settings.shortcuts.toggle_microphone",
        default: "ctrl-alt-m",
    },
    Action {
        key: "shortcut-new-session",
        field: "shortcutNewSession",
        label_key: "settings.shortcuts.new_session",
        default: "ctrl-alt-n",
    },
    Action {
        key: "shortcut-next-session",
        field: "shortcutNextSession",
        label_key: "settings.shortcuts.next_session",
        default: "ctrl-tab",
    },
    Action {
        key: "shortcut-prev-session",
        field: "shortcutPrevSession",
        label_key: "settings.shortcuts.prev_session",
        default: "ctrl-shift-tab",
    },
];

pub fn action(key: &str) -> Option<&'static Action> {
    ACTIONS.iter().find(|a| a.key == key)
}

#[derive(Debug, PartialEq)]
pub enum ShortcutError {
    Invalid(String),
    NoModifier(String),
    /// The combo, and the setting key of the action that already owns it.
    Duplicate(String, &'static str),
}

impl fmt::Display for ShortcutError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(c) => write!(f, "`{c}` is not a valid key combination"),
            Self::NoModifier(c) => write!(
                f,
                "`{c}` needs ctrl, alt or cmd; only F-keys can be used alone"
            ),
            Self::Duplicate(c, other) => write!(f, "`{c}` is already used by `{other}`"),
        }
    }
}

fn is_function_key(key: &str) -> bool {
    key.strip_prefix('f')
        .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

/// Canonical form (`ctrl-alt-m`) of a combo, if it is one a shortcut may use.
pub fn normalize(raw: &str) -> Result<String, ShortcutError> {
    let raw = raw.trim();
    let ks = Keystroke::parse(raw).map_err(|_| ShortcutError::Invalid(raw.into()))?;
    if ks.key.is_empty() {
        return Err(ShortcutError::Invalid(raw.into()));
    }
    let m = &ks.modifiers;
    if !(m.control || m.alt || m.platform || m.function || is_function_key(&ks.key)) {
        return Err(ShortcutError::NoModifier(raw.into()));
    }
    Ok(ks.unparse())
}

/// `normalize` plus the duplicate check; `current` gives the combo each action holds now.
pub fn validate(
    key: &str,
    raw: &str,
    current: impl Fn(&Action) -> String,
) -> Result<String, ShortcutError> {
    let combo = normalize(raw)?;
    match ACTIONS
        .iter()
        .find(|a| a.key != key && current(a).eq_ignore_ascii_case(&combo))
    {
        Some(other) => Err(ShortcutError::Duplicate(combo, other.key)),
        None => Ok(combo),
    }
}

impl Prefs {
    pub fn shortcut(&self, key: &str) -> &str {
        match key {
            "shortcut-toggle-fullscreen" => &self.shortcut_toggle_fullscreen,
            "shortcut-toggle-sidebar" => &self.shortcut_toggle_sidebar,
            "shortcut-open-skills" => &self.shortcut_open_skills,
            "shortcut-open-terminal" => &self.shortcut_open_terminal,
            "shortcut-toggle-microphone" => &self.shortcut_toggle_microphone,
            "shortcut-new-session" => &self.shortcut_new_session,
            "shortcut-next-session" => &self.shortcut_next_session,
            "shortcut-prev-session" => &self.shortcut_prev_session,
            _ => "",
        }
    }

    pub fn set_shortcut(&mut self, key: &str, combo: String) {
        let slot = match key {
            "shortcut-toggle-fullscreen" => &mut self.shortcut_toggle_fullscreen,
            "shortcut-toggle-sidebar" => &mut self.shortcut_toggle_sidebar,
            "shortcut-open-skills" => &mut self.shortcut_open_skills,
            "shortcut-open-terminal" => &mut self.shortcut_open_terminal,
            "shortcut-toggle-microphone" => &mut self.shortcut_toggle_microphone,
            "shortcut-new-session" => &mut self.shortcut_new_session,
            "shortcut-next-session" => &mut self.shortcut_next_session,
            "shortcut-prev-session" => &mut self.shortcut_prev_session,
            _ => return,
        };
        *slot = combo;
    }

    /// The combo an action is bound to: the stored one, or its default if the stored one is not
    /// usable (a hand-edited prefs file).
    pub fn effective_shortcut(&self, a: &Action) -> String {
        normalize(self.shortcut(a.key)).unwrap_or_else(|_| a.default.into())
    }

    pub fn effective_shortcuts(&self) -> Vec<String> {
        ACTIONS.iter().map(|a| self.effective_shortcut(a)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_valid_and_distinct() {
        let p = Prefs::default();
        let mut seen = std::collections::HashSet::new();
        for a in &ACTIONS {
            assert_eq!(normalize(a.default).unwrap(), a.default, "{}", a.key);
            assert_eq!(p.shortcut(a.key), a.default);
            assert!(seen.insert(a.default));
        }
    }

    #[test]
    fn rejects_bare_keys_garbage_and_duplicates() {
        assert!(matches!(normalize("m"), Err(ShortcutError::NoModifier(_))));
        assert!(matches!(
            normalize("shift-m"),
            Err(ShortcutError::NoModifier(_))
        ));
        assert_eq!(normalize("f5").unwrap(), "f5");
        assert_eq!(normalize("alt-ctrl-m").unwrap(), "ctrl-alt-m");
        assert!(matches!(normalize(""), Err(ShortcutError::Invalid(_))));
        let p = Prefs::default();
        let cur = |a: &Action| p.shortcut(a.key).to_string();
        let e = validate("shortcut-open-skills", "ctrl-alt-m", cur).unwrap_err();
        assert_eq!(
            e,
            ShortcutError::Duplicate("ctrl-alt-m".into(), "shortcut-toggle-microphone")
        );
        assert!(validate("shortcut-toggle-microphone", "ctrl-alt-m", cur).is_ok());
    }
}
