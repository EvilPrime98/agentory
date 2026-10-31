//! Minimal i18n: flat `"dotted.key" -> text` JSON files embedded under `assets/locales/`.
//!
//! `t("settings.language.title")` resolves at call time against the current language, so strings
//! must be looked up while rendering, never cached in structs or statics. A missing ES string
//! falls back to EN, then to the key itself. Switching language refreshes all windows (see
//! [`apply`]), which re-runs every `render`.

use std::collections::HashMap;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU8, Ordering};

use serde::{Deserialize, Serialize};

use crate::assets::Assets;

/// What the user picked in Settings.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LanguagePref {
    #[default]
    System,
    English,
    Spanish,
}

impl LanguagePref {
    /// Next value when cycling in Settings: System, English, Español.
    pub fn next(self) -> Self {
        match self {
            Self::System => Self::English,
            Self::English => Self::Spanish,
            Self::Spanish => Self::System,
        }
    }

    pub fn label_key(self) -> &'static str {
        match self {
            Self::System => "settings.language.system",
            Self::English => "settings.language.english",
            Self::Spanish => "settings.language.spanish",
        }
    }

    /// The concrete language this preference resolves to right now.
    pub fn resolve(self) -> Lang {
        match self {
            Self::English => Lang::En,
            Self::Spanish => Lang::Es,
            Self::System => detect_system(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    En = 0,
    Es = 1,
}

fn detect_system() -> Lang {
    match sys_locale::get_locale() {
        Some(l) if l.to_ascii_lowercase().starts_with("es") => Lang::Es,
        _ => Lang::En,
    }
}

type Table = HashMap<String, String>;

static CURRENT: AtomicU8 = AtomicU8::new(Lang::En as u8);

fn load(file: &str) -> Table {
    Assets::get(file)
        .and_then(|f| serde_json::from_slice(&f.data).ok())
        .unwrap_or_default()
}

fn en() -> &'static Table {
    static T: OnceLock<Table> = OnceLock::new();
    T.get_or_init(|| load("locales/en.json"))
}

fn es() -> &'static Table {
    static T: OnceLock<Table> = OnceLock::new();
    T.get_or_init(|| load("locales/es.json"))
}

pub fn current() -> Lang {
    if CURRENT.load(Ordering::Relaxed) == Lang::Es as u8 {
        Lang::Es
    } else {
        Lang::En
    }
}

/// Set the active language from the stored preference. Does not re-render by itself.
pub fn set(pref: LanguagePref) {
    CURRENT.store(pref.resolve() as u8, Ordering::Relaxed);
}

/// Set the language and re-render every window.
pub fn apply(pref: LanguagePref, cx: &mut gpui::App) {
    set(pref);
    cx.refresh_windows();
}

fn lookup(lang: Lang, key: &str) -> Option<&'static str> {
    let primary = match lang {
        Lang::En => en(),
        Lang::Es => es(),
    };
    primary
        .get(key)
        .or_else(|| en().get(key))
        .map(String::as_str)
}

/// Text for `key` in the current language.
pub fn t(key: &str) -> String {
    lookup(current(), key).unwrap_or(key).to_string()
}

/// Like [`t`], replacing each `{name}` with its value from `args`.
#[allow(dead_code)] // used once pages are extracted
pub fn t_args(key: &str, args: &[(&str, &str)]) -> String {
    substitute(lookup(current(), key).unwrap_or(key), args)
}

/// Plural lookup: `key_one` when `n == 1`, `key_other` otherwise. `{n}` is filled in.
#[allow(dead_code)] // used once pages are extracted
pub fn t_n(key: &str, n: usize) -> String {
    let suffix = if n == 1 { "one" } else { "other" };
    t_args(&format!("{key}_{suffix}"), &[("n", &n.to_string())])
}

#[allow(dead_code)]
fn substitute(text: &str, args: &[(&str, &str)]) -> String {
    let mut out = text.to_string();
    for (name, value) in args {
        out = out.replace(&format!("{{{name}}}"), value);
    }
    out
}

/// Names of the `{placeholders}` in `text`, sorted and deduplicated.
#[cfg(test)]
fn placeholders(text: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find('{') {
        let Some(len) = rest[start..].find('}') else { break };
        found.push(rest[start + 1..start + len].to_string());
        rest = &rest[start + len + 1..];
    }
    found.sort();
    found.dedup();
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locale_files_parse() {
        assert!(!en().is_empty(), "en.json missing or invalid");
        assert!(!es().is_empty(), "es.json missing or invalid");
    }

    #[test]
    fn es_has_every_en_key_with_same_placeholders() {
        let mut problems = Vec::new();
        for (key, en_text) in en() {
            match es().get(key) {
                None => problems.push(format!("missing in es.json: {key}")),
                Some(es_text) if placeholders(es_text) != placeholders(en_text) => {
                    problems.push(format!("placeholder mismatch: {key}"))
                }
                _ => {}
            }
        }
        for key in es().keys().filter(|k| !en().contains_key(*k)) {
            problems.push(format!("extra in es.json: {key}"));
        }
        assert!(problems.is_empty(), "{}", problems.join("\n"));
    }

    #[test]
    fn missing_keys_fall_back_to_en_then_key() {
        assert_eq!(
            lookup(Lang::Es, "settings.language.title"),
            Some("Idioma")
        );
        assert_eq!(lookup(Lang::Es, "no.such.key"), None);
        assert_eq!(t("no.such.key"), "no.such.key");
    }

    #[test]
    fn substitutes_placeholders() {
        assert_eq!(substitute("hi {name}, {name}!", &[("name", "Ana")]), "hi Ana, Ana!");
    }
}
