use serde_json::Value;

use crate::state::prefs::{MAX_FONT, MIN_FONT, Map, Prefs, to_map};

#[derive(Debug)]
pub enum Kind {
    Bool,
    Float { min: f32, max: f32 },
    Text,
    Choice(&'static [&'static str]),
}

#[derive(Debug)]
pub struct Setting {
    pub key: &'static str,
    pub field: &'static str,
    pub kind: Kind,
    pub description: &'static str,
}

pub const SETTINGS: &[Setting] = &[
    Setting {
        key: "theme",
        field: "theme",
        kind: Kind::Choice(&["system", "light", "dark"]),
        description: "Light or dark palette. system follows the OS appearance.",
    },
    Setting {
        key: "language",
        field: "language",
        kind: Kind::Choice(&["system", "english", "spanish"]),
        description: "Interface language. system follows the OS locale.",
    },
    Setting {
        key: "claude-command",
        field: "claudeCommand",
        kind: Kind::Text,
        description: "Program started in each agent session. Empty means `claude` from PATH.",
    },
    Setting {
        key: "ask-worktree",
        field: "askWorktree",
        kind: Kind::Bool,
        description: "Offer a git worktree when starting a session in a git project.",
    },
    Setting {
        key: "send-doc-prompt",
        field: "sendDocPrompt",
        kind: Kind::Bool,
        description: "Submit the prompt, not just type it, when running a doc from the Docs page.",
    },
    Setting {
        key: "doc-prompt",
        field: "docPrompt",
        kind: Kind::Text,
        description: "Prompt for running a doc from the Docs page. {doc} is the doc path under docs/. Empty restores the default.",
    },
    Setting {
        key: "notifications",
        field: "notifications",
        kind: Kind::Bool,
        description: "Toast when a session needs input or finishes.",
    },
    Setting {
        key: "fullscreen-agents",
        field: "fullscreenAgents",
        kind: Kind::Bool,
        description: "Launch agent sessions in Claude Code's fullscreen renderer.",
    },
    Setting {
        key: "terminal-font-size",
        field: "terminalFontSize",
        kind: Kind::Float {
            min: MIN_FONT,
            max: MAX_FONT,
        },
        description: "Terminal font size in points.",
    },
];

#[derive(Debug, PartialEq)]
pub struct SettingError {
    pub code: &'static str,
    pub message: String,
    pub allowed: Vec<String>,
}

pub fn find(key: &str) -> Result<&'static Setting, SettingError> {
    SETTINGS
        .iter()
        .find(|s| s.key == key)
        .ok_or_else(|| SettingError {
            code: "unknown_key",
            message: format!("unknown setting `{key}`"),
            allowed: SETTINGS.iter().map(|s| s.key.to_string()).collect(),
        })
}

impl Kind {
    pub fn name(&self) -> &'static str {
        match self {
            Kind::Bool => "bool",
            Kind::Float { .. } => "number",
            Kind::Text => "string",
            Kind::Choice(_) => "choice",
        }
    }
}

impl Setting {
    pub fn allowed(&self) -> Vec<String> {
        match &self.kind {
            Kind::Bool => vec!["true".into(), "false".into()],
            Kind::Float { min, max } => vec![format!("{min}..={max}")],
            Kind::Text => Vec::new(),
            Kind::Choice(c) => c.iter().map(|s| s.to_string()).collect(),
        }
    }

    pub fn default_value(&self) -> Value {
        to_map(&Prefs::default())
            .remove(self.field)
            .unwrap_or(Value::Null)
    }

    pub fn value_in(&self, file: &Map) -> Value {
        file.get(self.field)
            .cloned()
            .unwrap_or_else(|| self.default_value())
    }

    pub fn parse(&self, raw: &str) -> Result<Value, SettingError> {
        let invalid = |what: &str| SettingError {
            code: "invalid_value",
            message: format!("`{raw}` is not valid for `{}`: {what}", self.key),
            allowed: self.allowed(),
        };
        match &self.kind {
            Kind::Bool => match raw {
                "true" => Ok(Value::Bool(true)),
                "false" => Ok(Value::Bool(false)),
                _ => Err(invalid("expected true or false")),
            },
            Kind::Float { min, max } => {
                let n: f32 = raw
                    .trim()
                    .parse()
                    .map_err(|_| invalid("expected a number"))?;
                if !n.is_finite() || n < *min || n > *max {
                    return Err(invalid(&format!("out of range {min}..={max}")));
                }
                serde_json::Number::from_f64(f64::from(n))
                    .map(Value::Number)
                    .ok_or_else(|| invalid("expected a number"))
            }
            Kind::Text => Ok(Value::String(raw.to_string())),
            Kind::Choice(c) => c
                .contains(&raw)
                .then(|| Value::String(raw.to_string()))
                .ok_or_else(|| invalid("not one of the allowed values")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setting(key: &str) -> &'static Setting {
        find(key).unwrap()
    }

    #[test]
    fn rejects_out_of_range_and_unknown_values() {
        assert!(setting("terminal-font-size").parse("8").is_err());
        assert!(setting("terminal-font-size").parse("29").is_err());
        assert!(setting("terminal-font-size").parse("abc").is_err());
        assert!(setting("terminal-font-size").parse("14").is_ok());
        let e = setting("theme").parse("blue").unwrap_err();
        assert_eq!(e.code, "invalid_value");
        assert_eq!(e.allowed, ["system", "light", "dark"]);
        assert!(setting("notifications").parse("yes").is_err());
        assert_eq!(find("last-shell").unwrap_err().code, "unknown_key");
    }

    #[test]
    fn every_setting_maps_to_a_prefs_field_and_every_choice_deserializes() {
        let defaults = to_map(&Prefs::default());
        for s in SETTINGS {
            assert!(defaults.contains_key(s.field), "{} missing in Prefs", s.key);
            assert_eq!(
                s.key.replace('-', "").to_lowercase(),
                s.field.to_lowercase()
            );
            if let Kind::Choice(choices) = &s.kind {
                for c in *choices {
                    let mut m = Map::new();
                    m.insert(s.field.into(), Value::String(c.to_string()));
                    serde_json::from_value::<Prefs>(Value::Object(m))
                        .unwrap_or_else(|e| panic!("{} = {c}: {e}", s.key));
                }
            }
        }
    }

    #[test]
    fn app_state_fields_are_not_exposed() {
        for hidden in ["selfBuildEnabled", "sidebarCollapsed", "lastShell"] {
            assert!(SETTINGS.iter().all(|s| s.field != hidden));
        }
    }
}
