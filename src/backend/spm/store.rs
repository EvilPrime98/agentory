//! Skill folders, `spm.json`, `.claude/.spm-state.json` and `skillOverrides`: the on-disk formats
//! shared with the Node `spm` CLI.

use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use regex::Regex;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

pub(super) fn re(cell: &'static OnceLock<Regex>, pattern: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(pattern).expect("valid regex"))
}

/// Order of two names the way `localeCompare` sorts file names: case-insensitive first, `_ - .`
/// ahead of letters. An approximation of ICU, enough for the names skills use.
pub fn locale_cmp(a: &str, b: &str) -> Ordering {
    fn rank(c: char) -> (u8, char) {
        match c {
            '_' => (0, c),
            '-' => (1, c),
            '.' => (2, c),
            c if c.is_ascii_digit() => (3, c),
            c => (4, c.to_lowercase().next().unwrap_or(c)),
        }
    }
    a.chars()
        .map(rank)
        .cmp(b.chars().map(rank))
        // Lowercase before uppercase when they are otherwise equal.
        .then_with(|| b.cmp(a))
}

/// Non-hidden directories (or symlinks) of `dir` that hold a `SKILL.md`, sorted.
pub fn list_skills(dir: &Path) -> Vec<String> {
    let Ok(rd) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut found: Vec<String> = rd
        .filter_map(Result::ok)
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let path = e.path();
            (!name.starts_with('.') && path.is_dir() && path.join("SKILL.md").exists())
                .then_some(name)
        })
        .collect();
    found.sort_by(|a, b| locale_cmp(a, b));
    found
}

const SKIP: [&str; 3] = ["node_modules", "dist", "build"];
const MAX_DEPTH: usize = 6;

/// Skills in `root` and in any `skills/` folder below it, as (name, directory). A `root` that is
/// itself a skill is a single-skill repo. The shallowest match wins when names collide.
pub fn find_skills(root: &Path, root_name: &str) -> Vec<(String, PathBuf)> {
    static NAME: OnceLock<Regex> = OnceLock::new();
    let mut found: Vec<(String, PathBuf)> = Vec::new();
    if root.join("SKILL.md").exists() {
        let declared = read_name(root).unwrap_or_default();
        let name = if re(&NAME, r"^[\w.-]+$").is_match(&declared) {
            declared
        } else {
            root_name.to_string()
        };
        if !name.is_empty() {
            found.push((name, root.to_path_buf()));
        }
        return found;
    }
    let add = |found: &mut Vec<(String, PathBuf)>, dir: &Path| {
        for name in list_skills(dir) {
            if !found.iter().any(|(n, _)| *n == name) {
                let path = dir.join(&name);
                found.push((name, path));
            }
        }
    };
    add(&mut found, root);
    let mut level = vec![root.to_path_buf()];
    for _ in 0..MAX_DEPTH {
        if level.is_empty() {
            break;
        }
        let mut next = Vec::new();
        for dir in &level {
            let Ok(rd) = fs::read_dir(dir) else { continue };
            let mut entries: Vec<_> = rd.filter_map(Result::ok).collect();
            entries.sort_by(|a, b| {
                locale_cmp(&a.file_name().to_string_lossy(), &b.file_name().to_string_lossy())
            });
            for e in entries {
                let name = e.file_name().to_string_lossy().into_owned();
                // `isDirectory()` on a dirent does not follow symlinks.
                let is_dir = e.file_type().map(|t| t.is_dir()).unwrap_or(false);
                if !is_dir || name.starts_with('.') || SKIP.contains(&name.as_str()) {
                    continue;
                }
                let full = e.path();
                if full.join("SKILL.md").exists() {
                    continue;
                }
                if name == "skills" {
                    add(&mut found, &full);
                }
                next.push(full);
            }
        }
        level = next;
    }
    found
}

fn strip_quotes(s: &str) -> &str {
    let s = s.trim();
    let s = s.strip_prefix(['"', '\'']).unwrap_or(s);
    s.strip_suffix(['"', '\'']).unwrap_or(s)
}

/// The `name:` line of a `SKILL.md`.
pub fn read_name(dir: &Path) -> Option<String> {
    static NAME: OnceLock<Regex> = OnceLock::new();
    let text = fs::read_to_string(dir.join("SKILL.md")).ok()?;
    let m = re(&NAME, r"(?m)^name:\s*(.+)$").captures(&text)?;
    Some(strip_quotes(&m[1]).to_string())
}

/// The folder name plus the `name:` field of its `SKILL.md`.
pub fn skill_names(dir: &Path, folder: &str) -> Vec<String> {
    let mut names = vec![folder.to_string()];
    if let Some(n) = read_name(&dir.join(folder)) {
        if !names.contains(&n) {
            names.push(n);
        }
    }
    names
}

/// Fields of a `SKILL.md` front matter plus its body.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ParsedSkill {
    pub name: String,
    pub description: String,
    pub body: String,
}

/// Split a `SKILL.md` into the always-loaded front matter fields and the rest. Folded (`>`) and
/// literal (`|`) YAML scalars are joined into one line.
pub fn parse_skill(text: &str) -> ParsedSkill {
    static FRONT: OnceLock<Regex> = OnceLock::new();
    static MARKER: OnceLock<Regex> = OnceLock::new();
    let m = re(&FRONT, r"(?s)\A---\r?\n(.*?)\r?\n---\r?\n?").captures(text);
    let front = m.as_ref().map_or("", |m| m.get(1).unwrap().as_str());
    let field = |key: &str| -> String {
        let pattern = format!(r"(?m)^{key}:[ \t]*(.*)$((?:\r?\n[ \t]+.*)*)");
        let Some(c) = Regex::new(&pattern).unwrap().captures(front) else {
            return String::new();
        };
        let first = if re(&MARKER, r"^[>|][+-]?$").is_match(c[1].trim()) {
            ""
        } else {
            &c[1]
        };
        let joined = std::iter::once(first)
            .chain(c[2].split('\n'))
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        strip_quotes(&joined).to_string()
    };
    let body_at = m.map_or(0, |m| m.get(0).unwrap().end());
    ParsedSkill {
        name: field("name"),
        description: field("description"),
        body: text[body_at..].to_string(),
    }
}

/// First 16 hex characters of a SHA-256 over the relative paths and contents under `dir`
/// (sorted, `.git` ignored, symlinks hashed by target).
pub fn hash_dir(dir: &Path) -> std::io::Result<String> {
    fn walk(root: &Path, d: &Path, h: &mut Sha256) -> std::io::Result<()> {
        let mut entries: Vec<_> = fs::read_dir(d)?.collect::<Result<_, _>>()?;
        entries.sort_by(|a, b| {
            locale_cmp(&a.file_name().to_string_lossy(), &b.file_name().to_string_lossy())
        });
        for e in entries {
            if e.file_name() == ".git" {
                continue;
            }
            let full = e.path();
            let ty = e.file_type()?;
            if ty.is_dir() {
                walk(root, &full, h)?;
            } else {
                let rel = full.strip_prefix(root).unwrap_or(&full);
                h.update(rel.to_string_lossy().replace('\\', "/").as_bytes());
                if ty.is_symlink() {
                    let target = fs::read_link(&full)?;
                    h.update(format!("-> {}", target.to_string_lossy()).as_bytes());
                } else {
                    h.update(fs::read(&full)?);
                }
            }
        }
        Ok(())
    }
    let mut h = Sha256::new();
    walk(dir, dir, &mut h)?;
    Ok(hex(&h.finalize())[..16].to_string())
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Copy a skill folder over `dest`, replacing it. Symlinks are followed, `.git` is left out.
pub fn copy_skill(src: &Path, dest: &Path) -> std::io::Result<()> {
    fn copy(src: &Path, dest: &Path) -> std::io::Result<()> {
        fs::create_dir_all(dest)?;
        for e in fs::read_dir(src)? {
            let e = e?;
            if e.file_name() == ".git" {
                continue;
            }
            let (from, to) = (e.path(), dest.join(e.file_name()));
            if from.is_dir() {
                copy(&from, &to)?;
            } else {
                fs::copy(&from, &to)?;
            }
        }
        Ok(())
    }
    if dest.exists() {
        fs::remove_dir_all(dest)?;
    }
    copy(src, dest)
}

// ---- spm.json ------------------------------------------------------------------------------

/// `spm.json`. Keys other than `repos` and `skills` (`isolate`, anything a newer CLI adds) are
/// kept in `rest` and written back untouched.
#[derive(Debug, Clone, Default)]
pub struct Manifest {
    pub repos: Vec<String>,
    /// Skill name to its source: a repo spec, `null`, or a market entry object.
    pub skills: BTreeMap<String, Value>,
    pub rest: Map<String, Value>,
}

impl Manifest {
    pub fn isolate(&self) -> bool {
        self.rest.get("isolate").and_then(Value::as_bool).unwrap_or(false)
    }

    pub fn set_isolate(&mut self, on: bool) {
        self.rest.insert("isolate".into(), Value::Bool(on));
    }
}

fn read_json(file: &Path) -> Result<Option<Value>, String> {
    match fs::read_to_string(file) {
        Ok(s) => serde_json::from_str(&s)
            .map(Some)
            .map_err(|e| format!("could not parse {}: {e}", file.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("could not read {}: {e}", file.display())),
    }
}

fn write_json(file: &Path, data: &Value) -> Result<(), String> {
    if let Some(parent) = file.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let mut text = serde_json::to_string_pretty(data).map_err(|e| e.to_string())?;
    text.push('\n');
    fs::write(file, text).map_err(|e| format!("could not write {}: {e}", file.display()))
}

/// Read `spm.json`; a missing file is an empty manifest. The legacy format (`repo` string and a
/// `skills` array) is migrated on read.
pub fn read_manifest(file: &Path) -> Result<Manifest, String> {
    let Some(Value::Object(mut obj)) = read_json(file)? else {
        return Ok(Manifest::default());
    };
    let repo = obj.remove("repo");
    let repos = obj.remove("repos");
    let skills = obj.remove("skills");
    let mut pool: Vec<String> = Vec::new();
    let listed = repo
        .into_iter()
        .chain(repos.into_iter().flat_map(|r| match r {
            Value::Array(a) => a,
            _ => Vec::new(),
        }))
        .filter_map(|v| v.as_str().map(str::to_string))
        .filter(|s| !s.is_empty());
    for r in listed {
        if !pool.contains(&r) {
            pool.push(r);
        }
    }
    let skills = match skills {
        Some(Value::Array(names)) => names
            .into_iter()
            .filter_map(|n| n.as_str().map(str::to_string))
            .map(|n| (n, pool.first().map_or(Value::Null, |r| Value::String(r.clone()))))
            .collect(),
        Some(Value::Object(o)) => o.into_iter().collect(),
        _ => BTreeMap::new(),
    };
    Ok(Manifest {
        repos: pool,
        skills,
        rest: obj,
    })
}

pub fn write_manifest(file: &Path, m: &Manifest) -> Result<(), String> {
    let mut out = Map::new();
    let mut repos: Vec<&String> = Vec::new();
    for r in &m.repos {
        if !repos.contains(&r) {
            repos.push(r);
        }
    }
    out.insert("repos".into(), repos.into_iter().cloned().map(Value::String).collect());
    out.insert(
        "skills".into(),
        Value::Object(m.skills.iter().map(|(k, v)| (k.clone(), v.clone())).collect()),
    );
    for (k, v) in &m.rest {
        out.insert(k.clone(), v.clone());
    }
    write_json(file, &Value::Object(out))
}

// ---- .claude/.spm-state.json ---------------------------------------------------------------

/// What `spm` last copied for a skill, to tell later whether it was edited by hand.
pub type State = BTreeMap<String, Value>;

pub fn read_state(file: &Path) -> Result<State, String> {
    Ok(match read_json(file)? {
        Some(Value::Object(mut o)) => match o.remove("skills") {
            Some(Value::Object(s)) => s.into_iter().collect(),
            _ => State::new(),
        },
        _ => State::new(),
    })
}

pub fn write_state(file: &Path, state: &State) -> Result<(), String> {
    let skills: Map<String, Value> = state.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    let mut out = Map::new();
    out.insert("skills".into(), Value::Object(skills));
    write_json(file, &Value::Object(out))
}

pub fn state_entry(hash: &str, sha: &str) -> Value {
    serde_json::json!({ "hash": hash, "sha": sha })
}

pub fn state_hash(state: &State, name: &str) -> Option<String> {
    state.get(name)?.get("hash")?.as_str().map(str::to_string)
}

// ---- .claude/settings.json ----------------------------------------------------------------

fn read_settings(file: &Path) -> Result<Map<String, Value>, String> {
    match read_json(file)? {
        Some(Value::Object(o)) => Ok(o),
        _ => Ok(Map::new()),
    }
}

/// Hide global skills in the project: each gets `"off"` in `skillOverrides`, unless a local skill
/// has the same name (an existing `"off"` is then removed).
pub fn sync_overrides(settings_file: &Path, global_dir: &Path, local_dir: &Path) -> Result<(), String> {
    let mut settings = read_settings(settings_file)?;
    let mut keep: Vec<String> = Vec::new();
    for folder in list_skills(local_dir) {
        keep.extend(skill_names(local_dir, &folder));
    }
    let mut overrides = match settings.get("skillOverrides") {
        Some(Value::Object(o)) => o.clone(),
        _ => Map::new(),
    };
    for folder in list_skills(global_dir) {
        for n in skill_names(global_dir, &folder) {
            if keep.contains(&n) {
                if overrides.get(&n).and_then(Value::as_str) == Some("off") {
                    overrides.shift_remove(&n);
                }
            } else {
                overrides.insert(n, Value::String("off".into()));
            }
        }
    }
    settings.insert("skillOverrides".into(), Value::Object(overrides));
    write_json(settings_file, &Value::Object(settings))
}

/// Undo `sync_overrides`: drop the `"off"` entries for global skills, and `skillOverrides`
/// itself once empty. Does nothing when the file is missing or unreadable.
pub fn remove_overrides(settings_file: &Path, global_dir: &Path) -> Result<(), String> {
    let Ok(Some(Value::Object(mut settings))) = read_json(settings_file) else {
        return Ok(());
    };
    let Some(Value::Object(overrides)) = settings.get_mut("skillOverrides") else {
        return Ok(());
    };
    for folder in list_skills(global_dir) {
        for n in skill_names(global_dir, &folder) {
            if overrides.get(&n).and_then(Value::as_str) == Some("off") {
                overrides.shift_remove(&n);
            }
        }
    }
    if overrides.is_empty() {
        settings.shift_remove("skillOverrides");
    }
    write_json(settings_file, &Value::Object(settings))
}

/// `skillOverrides` of the project settings; empty when missing or unreadable.
pub fn read_overrides(settings_file: &Path) -> BTreeMap<String, String> {
    let Ok(Some(Value::Object(mut s))) = read_json(settings_file) else {
        return BTreeMap::new();
    };
    match s.remove("skillOverrides") {
        Some(Value::Object(o)) => o
            .into_iter()
            .filter_map(|(k, v)| v.as_str().map(|v| (k, v.to_string())))
            .collect(),
        _ => BTreeMap::new(),
    }
}
