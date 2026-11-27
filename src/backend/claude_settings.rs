use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::assets::Assets;

const FILE_NAME: &str = "settings.json";
const CONFIG_DIR_ENV: &str = "CLAUDE_CONFIG_DIR";
const CATALOG: &str = "claude_settings_catalog.json";
const SECRET_MARKS: [&str; 6] = ["KEY", "TOKEN", "SECRET", "PASSWORD", "HEADERS", "PROXY"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Bool,
    Enum,
    String,
    Multiline,
    Number,
    List,
    Json,
    Object,
    Env,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Shape {
    Object,
    Array,
    String,
    Boolean,
}

impl Shape {
    fn matches(self, value: &Value) -> bool {
        match self {
            Self::Object => value.is_object(),
            Self::Array => value.is_array(),
            Self::String => value.is_string(),
            Self::Boolean => value.is_boolean(),
        }
    }
}

pub fn fits(shapes: &[Shape], value: &Value) -> bool {
    shapes.is_empty() || shapes.iter().any(|s| s.matches(value))
}

#[derive(Debug, Deserialize)]
pub struct Setting {
    pub key: String,
    pub topic: String,
    pub kind: Kind,
    #[serde(default)]
    pub values: Vec<String>,
    #[serde(default)]
    pub suggestions: Vec<String>,
    #[serde(default)]
    pub integer: bool,
    #[serde(default)]
    pub shapes: Vec<Shape>,
    pub description: String,
    #[serde(default, rename = "type")]
    pub type_hint: String,
    #[serde(default)]
    pub default: String,
    pub example: Option<Value>,
    #[serde(default)]
    pub deprecated: bool,
}

impl Setting {
    pub fn path(&self) -> Vec<String> {
        self.key.split('.').map(str::to_string).collect()
    }

    pub fn accepts(&self, value: &Value) -> bool {
        match self.kind {
            Kind::Bool => value.is_boolean(),
            Kind::Enum => value
                .as_str()
                .is_some_and(|v| self.values.iter().any(|known| known == v)),
            Kind::String | Kind::Multiline => value.is_string(),
            Kind::Number => value.is_number() && (!self.integer || !value.is_f64()),
            Kind::List => value
                .as_array()
                .is_some_and(|items| items.iter().all(Value::is_string)),
            Kind::Json | Kind::Object | Kind::Env => fits(&self.shapes, value),
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct EnvVar {
    pub name: String,
    pub description: String,
}

#[derive(Debug, Deserialize)]
pub struct Catalog {
    pub topics: Vec<String>,
    pub settings: Vec<Setting>,
    pub env: Vec<EnvVar>,
}

impl Catalog {
    pub fn setting(&self, key: &str) -> Option<&Setting> {
        self.settings.iter().find(|s| s.key == key)
    }

    pub fn env_var(&self, name: &str) -> Option<&EnvVar> {
        self.env.iter().find(|v| v.name == name)
    }

    pub fn unknown_keys(&self, root: &Map<String, Value>) -> Vec<Vec<String>> {
        let mut found = Vec::new();
        self.collect_unknown(root, &mut Vec::new(), &mut found);
        found
    }

    fn collect_unknown(
        &self,
        map: &Map<String, Value>,
        parent: &mut Vec<String>,
        found: &mut Vec<Vec<String>>,
    ) {
        for (key, value) in map {
            parent.push(key.clone());
            let dotted = parent.join(".");
            let literal_dot = key.contains('.');
            let setting = self.setting(&dotted).filter(|_| !literal_dot);
            let descend = !literal_dot
                && match setting {
                    Some(s) => s.kind == Kind::Object,
                    None => {
                        let prefix = format!("{dotted}.");
                        self.settings.iter().any(|s| s.key.starts_with(&prefix))
                    }
                };
            match value.as_object() {
                Some(child) if descend => self.collect_unknown(child, parent, found),
                _ if setting.is_none() => found.push(parent.clone()),
                _ => {}
            }
            parent.pop();
        }
    }
}

pub fn catalog() -> &'static Catalog {
    static CELL: OnceLock<Catalog> = OnceLock::new();
    CELL.get_or_init(|| {
        let file = Assets::get(CATALOG).expect("the settings catalog is embedded");
        serde_json::from_slice(&file.data).expect("the settings catalog is valid")
    })
}

pub fn is_secret(name: &str) -> bool {
    let name = name.to_ascii_uppercase();
    SECRET_MARKS.iter().any(|mark| name.contains(mark))
}

pub fn path() -> Option<PathBuf> {
    path_in(std::env::var_os(CONFIG_DIR_ENV), dirs::home_dir())
}

fn path_in(config_dir: Option<OsString>, home: Option<PathBuf>) -> Option<PathBuf> {
    let dir = match config_dir {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir),
        _ => home?.join(".claude"),
    };
    Some(dir.join(FILE_NAME))
}

pub fn read_text(path: &Path) -> Result<Option<String>, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}

pub fn parse(text: &str) -> Result<Map<String, Value>, String> {
    let text = text.trim_start_matches('\u{feff}');
    if text.trim().is_empty() {
        return Ok(Map::new());
    }
    match serde_json::from_str(text).map_err(|e| e.to_string())? {
        Value::Object(map) => Ok(map),
        _ => Err("not a JSON object".to_string()),
    }
}

pub fn get<'a>(root: &'a Map<String, Value>, path: &[String]) -> Option<&'a Value> {
    let (first, rest) = path.split_first()?;
    rest.iter()
        .try_fold(root.get(first)?, |value, key| value.as_object()?.get(key))
}

fn insert(root: &mut Map<String, Value>, path: &[String], value: Value) -> Result<(), String> {
    let (last, parents) = path.split_last().ok_or("empty key")?;
    let mut map = root;
    for (depth, key) in parents.iter().enumerate() {
        map = map
            .entry(key.clone())
            .or_insert_with(|| Value::Object(Map::new()))
            .as_object_mut()
            .ok_or_else(|| format!("{} is not an object", path[..=depth].join(".")))?;
    }
    map.insert(last.clone(), value);
    Ok(())
}

fn delete(map: &mut Map<String, Value>, path: &[String]) -> bool {
    match path {
        [] => false,
        [key] => map.shift_remove(key).is_some(),
        [key, rest @ ..] => {
            let Some(child) = map.get_mut(key).and_then(Value::as_object_mut) else {
                return false;
            };
            let removed = delete(child, rest);
            if removed && child.is_empty() {
                map.shift_remove(key);
            }
            removed
        }
    }
}

pub fn update(
    file: &Path,
    key: &[String],
    change: impl FnOnce(Option<&Value>) -> Result<Option<Value>, String>,
) -> Result<(), String> {
    let text = read_text(file)?;
    let mut root = parse(text.as_deref().unwrap_or_default())?;
    match change(get(&root, key))? {
        Some(value) => insert(&mut root, key, value)?,
        None => {
            if !delete(&mut root, key) {
                return Ok(());
            }
        }
    }
    write(file, &root, text.as_deref()).map_err(|e| e.to_string())
}

fn indent_of(text: &str) -> Option<&str> {
    text.lines().find_map(|line| {
        let width = line.len() - line.trim_start_matches([' ', '\t']).len();
        (width > 0).then(|| &line[..width])
    })
}

fn write(file: &Path, root: &Map<String, Value>, previous: Option<&str>) -> std::io::Result<()> {
    let target = std::fs::canonicalize(file).unwrap_or_else(|_| file.to_path_buf());
    if let Some(dir) = target.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let indent = previous.and_then(indent_of).unwrap_or("  ");
    let mut json = Vec::new();
    let format = serde_json::ser::PrettyFormatter::with_indent(indent.as_bytes());
    root.serialize(&mut serde_json::Serializer::with_formatter(
        &mut json, format,
    ))
    .map_err(std::io::Error::other)?;
    if previous.is_none_or(|text| text.ends_with('\n')) {
        json.push(b'\n');
    }
    let tmp = target.with_extension(format!("json.tmp{}", std::process::id()));
    let written = (|| {
        if previous.is_some() {
            std::fs::copy(&target, &tmp)?;
        }
        std::fs::write(&tmp, &json)?;
        std::fs::rename(&tmp, &target)
    })();
    written.inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn temp_file(name: &str, content: Option<&str>) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "agentory-claude-settings-{}-{name}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let file = dir.join(FILE_NAME);
        if let Some(content) = content {
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(&file, content).unwrap();
        }
        file
    }

    fn key(dotted: &str) -> Vec<String> {
        dotted.split('.').map(str::to_string).collect()
    }

    fn set(file: &Path, dotted: &str, value: Value) -> Result<(), String> {
        update(file, &key(dotted), |_| Ok(Some(value)))
    }

    fn remove(file: &Path, dotted: &str) -> Result<(), String> {
        update(file, &key(dotted), |_| Ok(None))
    }

    fn text(file: &Path) -> String {
        std::fs::read_to_string(file).unwrap()
    }

    #[test]
    fn catalog_is_consistent() {
        let catalog = catalog();
        assert!(!catalog.settings.is_empty());
        assert!(!catalog.env.is_empty());
        for (i, s) in catalog.settings.iter().enumerate() {
            assert!(catalog.topics.contains(&s.topic), "{}", s.key);
            assert!(!s.description.is_empty(), "{}", s.key);
            assert_eq!(s.kind == Kind::Enum, !s.values.is_empty(), "{}", s.key);
            assert!(
                catalog.settings[..i].iter().all(|o| o.key != s.key),
                "duplicate {}",
                s.key
            );
            if let Some(example) = &s.example {
                assert!(s.accepts(example), "example of {}", s.key);
            }
        }
        assert_eq!(catalog.setting("env").map(|s| s.kind), Some(Kind::Env));
    }

    #[test]
    fn every_topic_is_translated() {
        let topics = catalog().topics.iter().map(String::as_str).chain(["other"]);
        for topic in topics {
            let id = format!("claude_settings.topic.{topic}");
            for locale in ["locales/en.json", "locales/es.json"] {
                let table: Map<String, Value> =
                    serde_json::from_slice(&Assets::get(locale).unwrap().data).unwrap();
                assert!(table.contains_key(&id), "{id} in {locale}");
            }
        }
    }

    #[test]
    fn accepts_checks_the_documented_type() {
        let accepts = |k: &str, v: Value| catalog().setting(k).unwrap().accepts(&v);
        assert!(accepts("alwaysThinkingEnabled", json!(false)));
        assert!(!accepts("alwaysThinkingEnabled", json!("false")));
        assert!(accepts("effortLevel", json!("high")));
        assert!(!accepts("effortLevel", json!("huge")));
        assert!(accepts("cleanupPeriodDays", json!(30)));
        assert!(!accepts("cleanupPeriodDays", json!(1.5)));
        assert!(accepts("feedbackSurveyRate", json!(0.5)));
        assert!(!accepts("permissions.allow", json!(["Bash", 1])));
        assert!(accepts("attribution", json!(false)));
        assert!(!accepts("hooks", json!([])));
    }

    #[test]
    fn unknown_keys_skip_what_a_row_covers() {
        let root = parse(
            r#"{
                "model": "opus",
                "invented": 1,
                "sandbox": {"enabled": true, "extra": 1, "network": {"more": 2}},
                "autoMode": {"allow": []},
                "remote": {"defaultEnvironmentId": "e", "odd": 3},
                "env": {"ANY": "1"},
                "sandbox.enabled": true
            }"#,
        )
        .unwrap();
        let found: Vec<String> = catalog()
            .unknown_keys(&root)
            .iter()
            .map(|p| p.join("/"))
            .collect();
        assert_eq!(
            found,
            [
                "invented",
                "sandbox/extra",
                "sandbox/network/more",
                "remote/odd",
                "sandbox.enabled"
            ]
        );
    }

    #[test]
    fn writes_change_only_the_edited_key_and_keep_order() {
        let file = temp_file(
            "order",
            Some(
                "{\n    \"zeta\": 1,\n    \"model\": \"a\",\n    \"alpha\": 2,\n    \"last\": 3\n}",
            ),
        );
        set(&file, "model", json!("b")).unwrap();
        assert_eq!(
            text(&file),
            "{\n    \"zeta\": 1,\n    \"model\": \"b\",\n    \"alpha\": 2,\n    \"last\": 3\n}"
        );
        remove(&file, "model").unwrap();
        assert_eq!(
            text(&file),
            "{\n    \"zeta\": 1,\n    \"alpha\": 2,\n    \"last\": 3\n}"
        );
    }

    #[test]
    fn nested_keys_are_created_and_pruned() {
        let file = temp_file("nested", None);
        set(&file, "sandbox.network.allowedDomains", json!(["a.com"])).unwrap();
        set(&file, "sandbox.enabled", json!(true)).unwrap();
        assert!(text(&file).ends_with("}\n"));
        remove(&file, "sandbox.network.allowedDomains").unwrap();
        assert_eq!(
            parse(&text(&file)).unwrap(),
            parse(r#"{"sandbox":{"enabled":true}}"#).unwrap()
        );
        remove(&file, "sandbox.enabled").unwrap();
        assert_eq!(text(&file), "{}\n");
    }

    #[test]
    fn change_sees_the_value_now_in_the_file() {
        let file = temp_file("current", Some(r#"{"permissions":{"allow":["a"]}}"#));
        update(&file, &key("permissions.allow"), |current| {
            let mut items = current.unwrap().as_array().unwrap().clone();
            items.push(json!("b"));
            Ok(Some(items.into()))
        })
        .unwrap();
        let root = parse(&text(&file)).unwrap();
        assert_eq!(
            get(&root, &key("permissions.allow")),
            Some(&json!(["a", "b"]))
        );
    }

    #[test]
    fn invalid_files_and_wrong_parents_are_left_alone() {
        for (name, content) in [("syntax", "{ nope"), ("array", "[1]")] {
            let file = temp_file(name, Some(content));
            assert!(set(&file, "model", json!("x")).is_err());
            assert!(remove(&file, "model").is_err());
            assert_eq!(text(&file), content);
        }
        let content = r#"{"attribution":false}"#;
        let file = temp_file("parent", Some(content));
        assert!(set(&file, "attribution.commit", json!("x")).is_err());
        remove(&file, "attribution.commit").unwrap();
        assert_eq!(text(&file), content);
    }

    #[test]
    fn removing_a_missing_key_creates_nothing() {
        let file = temp_file("missing", None);
        remove(&file, "model").unwrap();
        assert!(!file.exists());
    }

    #[cfg(unix)]
    #[test]
    fn writes_keep_links_and_permissions() {
        use std::os::unix::fs::PermissionsExt as _;

        let real = temp_file("link", Some("{}"));
        std::fs::set_permissions(&real, std::fs::Permissions::from_mode(0o600)).unwrap();
        let link = real.with_file_name("linked.json");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        set(&link, "model", json!("x")).unwrap();
        assert!(std::fs::symlink_metadata(&link).unwrap().is_symlink());
        assert!(text(&real).contains("model"));
        let mode = std::fs::metadata(&real).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn config_dir_override_wins() {
        let home = Some(PathBuf::from("home"));
        assert_eq!(
            path_in(Some("custom".into()), home.clone()),
            Some(Path::new("custom").join(FILE_NAME))
        );
        assert_eq!(
            path_in(Some("".into()), home),
            Some(Path::new("home").join(".claude").join(FILE_NAME))
        );
    }

    #[test]
    fn secret_names() {
        assert!(is_secret("ANTHROPIC_API_KEY"));
        assert!(is_secret("https_proxy"));
        assert!(is_secret("ANTHROPIC_CUSTOM_HEADERS"));
        assert!(!is_secret("ANTHROPIC_MODEL"));
    }
}
